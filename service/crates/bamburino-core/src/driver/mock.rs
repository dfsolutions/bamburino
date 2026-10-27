// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-memory [`DriverClient`] for tests (feature `mock`).
//!
//! Mirrors the observable behaviour of the real driver that the service
//! depends on: whitelists, a fresh `plateRevision` per `plate_list` that goes
//! stale on any edit, a modal-dialog gate on mutations, undo snapshots, and a
//! slice job that needs two polls to succeed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use super::map::{OVERRIDE_KEYS, WRITABLE_KEYS};
use super::{
    AbsoluteTransform, ActivePresets, AppState, DriverCapabilities, DriverClient, DriverError,
    DriverJob, DriverJobError, DriverTool, EffectiveSetting, FilamentUsage, Instance, JobStatus,
    ModelObject, Plate, PlateList, SettingSource, SettingsTarget, SliceState, SliceSummary,
};
use crate::build_info;
use crate::catalog::{Catalog, OptionKind};

const OBJECT_ID: &str = "obj-1";
const INSTANCE_ID: &str = "inst-1";

#[derive(Clone)]
struct Snapshot {
    settings: BTreeMap<String, String>,
    plate_overrides: BTreeMap<String, String>,
    object_overrides: BTreeMap<String, String>,
    instance: Instance,
}

#[derive(Default)]
struct SummaryStats {
    print_time_s: Option<f64>,
    filaments: Vec<FilamentUsage>,
    support_volume_mm3: Option<f64>,
}

struct Job {
    job: DriverJob,
}

struct State {
    calls: Vec<String>,
    // identity
    app_version: String,
    patch_id: Option<String>,
    session_id: String,
    // injected faults
    unreachable: bool,
    modal_open: bool,
    fail_next: Option<DriverError>,
    delay_next: Option<Duration>,
    // editor state
    settings: BTreeMap<String, String>,
    plate_overrides: BTreeMap<String, String>,
    object_overrides: BTreeMap<String, String>,
    instance: Instance,
    dirty: bool,
    // undo
    snapshots: Vec<Snapshot>,
    snapshot_time: u64,
    undo_snapshot: Option<String>,
    // plate revisions: revision string -> edit epoch at issue time
    epoch: u64,
    revision_counter: u64,
    issued: BTreeMap<String, u64>,
    // slicing
    job_counter: u64,
    jobs: BTreeMap<String, Job>,
    valid_result: Option<(String, u32)>,
    summary_stats: SummaryStats,
    estimated_time_s: Option<f64>,
}

/// Scriptable in-memory driver. All methods take `&self`.
pub struct MockDriver {
    state: Mutex<State>,
}

impl Default for MockDriver {
    fn default() -> Self {
        Self::new()
    }
}

fn default_string(key: &str) -> String {
    let Some(def) = Catalog::embedded().get(key) else {
        return "0".to_string();
    };
    let value = match &def.default {
        Some(serde_json::Value::Array(items)) => items.first().cloned(),
        other => other.clone(),
    };
    match value {
        Some(serde_json::Value::Bool(b)) => (if b { "1" } else { "0" }).to_string(),
        Some(serde_json::Value::Number(n)) if def.kind == OptionKind::Percent => format!("{n}%"),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::String(s)) => s,
        _ => "0".to_string(),
    }
}

fn cube_instance() -> Instance {
    Instance {
        id: INSTANCE_ID.to_string(),
        index: 0,
        position_mm: [128.0, 128.0, 10.0],
        rotation_deg: [0.0; 3],
        scale: [1.0; 3],
        plate_index: 0,
    }
}

impl State {
    fn object(&self) -> ModelObject {
        let s = self.instance.scale;
        ModelObject {
            id: OBJECT_ID.to_string(),
            index: 0,
            name: "cube".to_string(),
            dimensions_mm: [20.0 * s[0], 20.0 * s[1], 20.0 * s[2]],
            instances: vec![self.instance.clone()],
        }
    }

    fn slicing_running(&self) -> bool {
        self.jobs.values().any(|j| !j.job.status.is_terminal())
    }

    /// An edit invalidates the current slice result and active jobs.
    fn invalidate_slice(&mut self) {
        self.valid_result = None;
        for entry in self.jobs.values_mut() {
            if !entry.job.status.is_terminal() {
                entry.job.status = JobStatus::Stale;
                entry.job.error = Some(DriverJobError {
                    code: "STALE_REFERENCE".to_string(),
                    message: "slice result invalidated by an edit".to_string(),
                });
            }
        }
    }

    /// Issues a fresh plate revision for the current edit epoch.
    fn plate_state(&mut self) -> PlateList {
        self.revision_counter += 1;
        let revision = format!("mock-session:{}", self.revision_counter);
        self.issued.insert(revision.clone(), self.epoch);
        PlateList {
            plates: vec![Plate {
                index: 0,
                name: "Plate 1".to_string(),
                locked: false,
            }],
            selected_index: 0,
            plate_revision: revision,
        }
    }

    /// Records an undo snapshot of the state before a mutation and
    /// invalidates every issued plate revision.
    fn take_snapshot(&mut self) {
        self.snapshots.push(Snapshot {
            settings: self.settings.clone(),
            plate_overrides: self.plate_overrides.clone(),
            object_overrides: self.object_overrides.clone(),
            instance: self.instance.clone(),
        });
        self.snapshot_time += 1;
        self.undo_snapshot = Some(self.snapshot_time.to_string());
        self.epoch += 1;
        self.dirty = true;
        self.invalidate_slice();
    }

    fn check_revision(&self, revision: &str) -> Result<(), DriverError> {
        match self.issued.get(revision) {
            Some(epoch) if *epoch == self.epoch => Ok(()),
            Some(_) => Err(DriverError::StaleReference {
                message: format!("plate revision {revision} is outdated"),
            }),
            None => Err(DriverError::StaleReference {
                message: format!("plate revision {revision} was never issued"),
            }),
        }
    }

    fn check_plate(index: u32) -> Result<(), DriverError> {
        if index == 0 {
            Ok(())
        } else {
            Err(DriverError::InvalidParams {
                message: format!("plate index {index} out of range"),
            })
        }
    }
}

fn check_keys(values: &BTreeMap<String, String>, allowed: &[&str]) -> Result<(), DriverError> {
    if values.is_empty() || values.len() > 100 {
        return Err(DriverError::InvalidParams {
            message: "values must contain 1-100 entries".to_string(),
        });
    }
    for key in values.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(DriverError::InvalidParams {
                message: format!("setting {key} is not writable by the driver"),
            });
        }
    }
    Ok(())
}

impl MockDriver {
    pub fn new() -> Self {
        let settings = WRITABLE_KEYS
            .iter()
            .map(|k| (k.to_string(), default_string(k)))
            .collect();
        Self {
            state: Mutex::new(State {
                calls: Vec::new(),
                app_version: build_info::STUDIO_VERSION.to_string(),
                patch_id: build_info::EXPECTED_PATCH_ID.map(str::to_string),
                session_id: "mock-session".to_string(),
                unreachable: false,
                modal_open: false,
                fail_next: None,
                delay_next: None,
                settings,
                plate_overrides: BTreeMap::new(),
                object_overrides: BTreeMap::new(),
                instance: cube_instance(),
                dirty: false,
                snapshots: Vec::new(),
                snapshot_time: 0,
                undo_snapshot: None,
                epoch: 0,
                revision_counter: 0,
                issued: BTreeMap::new(),
                job_counter: 0,
                jobs: BTreeMap::new(),
                valid_result: None,
                summary_stats: SummaryStats::default(),
                estimated_time_s: Some(1800.0),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("mock state poisoned")
    }

    /// Names of the driver tools called so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    /// Simulates an edit made by the user in the GUI (takes an undo snapshot).
    pub fn user_edit(&self, key: &str, value: &str) {
        let mut st = self.lock();
        st.take_snapshot();
        st.settings.insert(key.to_string(), value.to_string());
    }

    /// Simulates the user pressing undo: restores the last snapshot and
    /// changes `undo_snapshot`.
    pub fn user_undo(&self) {
        let mut st = self.lock();
        if let Some(snap) = st.snapshots.pop() {
            st.settings = snap.settings;
            st.plate_overrides = snap.plate_overrides;
            st.object_overrides = snap.object_overrides;
            st.instance = snap.instance;
        }
        st.snapshot_time += 1;
        st.undo_snapshot = Some(st.snapshot_time.to_string());
        st.epoch += 1;
        st.invalidate_slice();
    }

    /// Simulates Task 18 data: print time, filament usage and support volume
    /// reported by `preview_summary`. Defaults to nothing, like the driver.
    pub fn set_summary_stats(
        &self,
        print_time_s: Option<f64>,
        filaments: Vec<FilamentUsage>,
        support_volume_mm3: Option<f64>,
    ) {
        self.lock().summary_stats = SummaryStats {
            print_time_s,
            filaments,
            support_volume_mm3,
        };
    }

    /// Overrides `statistics.estimatedNormalTime` reported by `slice_state`.
    pub fn set_estimated_time(&self, seconds: Option<f64>) {
        self.lock().estimated_time_s = seconds;
    }

    pub fn set_modal(&self, open: bool) {
        self.lock().modal_open = open;
    }

    /// The next call (of any kind) fails with `err`.
    pub fn fail_next(&self, err: DriverError) {
        self.lock().fail_next = Some(err);
    }

    /// The next call (of any kind) sleeps for `d` before running.
    pub fn delay_next(&self, d: Duration) {
        self.lock().delay_next = Some(d);
    }

    pub fn set_unreachable(&self, unreachable: bool) {
        self.lock().unreachable = unreachable;
    }

    /// Simulates a Studio restart: a new `session_id` is reported.
    pub fn set_session(&self, session_id: &str) {
        self.lock().session_id = session_id.to_string();
    }

    /// Overrides the reported identity (for version/patch mismatch tests).
    pub fn set_identity(&self, app_version: &str, patch_id: &str) {
        let mut st = self.lock();
        st.app_version = app_version.to_string();
        st.patch_id = Some(patch_id.to_string());
    }

    /// Common prologue: records the call, applies injected faults, enforces
    /// the mutation gate, and returns the locked state.
    async fn enter(&self, tool: DriverTool) -> Result<MutexGuard<'_, State>, DriverError> {
        let (delay, fail) = {
            let mut st = self.lock();
            st.calls.push(tool.name().to_string());
            if st.unreachable {
                return Err(DriverError::Unreachable);
            }
            (st.delay_next.take(), st.fail_next.take())
        };
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        if let Some(err) = fail {
            return Err(err);
        }
        let st = self.lock();
        if tool.is_mutation() && (st.modal_open || st.slicing_running()) {
            return Err(DriverError::Busy {
                message: "a modal dialog is open or a job is running".to_string(),
            });
        }
        Ok(st)
    }
}

#[async_trait::async_trait]
impl DriverClient for MockDriver {
    async fn app_state(&self) -> Result<AppState, DriverError> {
        let st = self.enter(DriverTool::AppGetState).await?;
        Ok(AppState {
            session_id: st.session_id.clone(),
            app_version: st.app_version.clone(),
            project_name: "Untitled".to_string(),
            project_path: String::new(),
            dirty: st.dirty,
            presets_dirty: st.dirty,
            ui_job_running: false,
            slicing_running: st.slicing_running(),
            modal_open: st.modal_open,
            object_count: 1,
            plate_count: 1,
            selected_plate_index: 0,
            undo_snapshot: st.undo_snapshot.clone(),
        })
    }

    async fn capabilities(&self) -> Result<DriverCapabilities, DriverError> {
        let st = self.enter(DriverTool::AppGetCapabilities).await?;
        let raw = serde_json::json!({
            "sessionId": st.session_id,
            "protocolVersion": 1,
            "appVersion": st.app_version,
            "buildSuffix": "bamburino",
            "patchId": st.patch_id,
        });
        Ok(DriverCapabilities {
            app_version: st.app_version.clone(),
            build_suffix: Some("bamburino".to_string()),
            patch_id: st.patch_id.clone(),
            raw,
        })
    }

    async fn plate_list(&self) -> Result<PlateList, DriverError> {
        let mut st = self.enter(DriverTool::PlateList).await?;
        Ok(st.plate_state())
    }

    async fn plate_select(
        &self,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<PlateList, DriverError> {
        let mut st = self.enter(DriverTool::PlateSelect).await?;
        st.check_revision(plate_revision)?;
        State::check_plate(plate_index)?;
        // Like the driver, selecting a plate takes an undo snapshot and so
        // invalidates every previously issued plate revision.
        st.take_snapshot();
        Ok(st.plate_state())
    }

    async fn model_list(&self) -> Result<Vec<ModelObject>, DriverError> {
        let st = self.enter(DriverTool::ModelList).await?;
        Ok(vec![st.object()])
    }

    async fn active_presets(&self) -> Result<ActivePresets, DriverError> {
        // The driver needs one `preset_list` call per kind.
        let mut st = self.enter(DriverTool::PresetList).await?;
        st.calls.push(DriverTool::PresetList.name().to_string());
        st.calls.push(DriverTool::PresetList.name().to_string());
        Ok(ActivePresets {
            printer: Some("Mock Printer".to_string()),
            process: Some("0.20mm Standard".to_string()),
            filaments: vec!["Mock PLA".to_string()],
        })
    }

    async fn writable_keys(&self) -> Result<BTreeSet<String>, DriverError> {
        let _st = self.enter(DriverTool::SettingsGet).await?;
        Ok(WRITABLE_KEYS.iter().map(|k| k.to_string()).collect())
    }

    async fn settings_effective(
        &self,
        target: &SettingsTarget,
        keys: Option<&[String]>,
    ) -> Result<Vec<EffectiveSetting>, DriverError> {
        let st = self.enter(DriverTool::SettingsEffective).await?;
        let (overrides, source) = match target {
            SettingsTarget::Project => (None, SettingSource::Project),
            SettingsTarget::Plate {
                index,
                plate_revision,
            } => {
                st.check_revision(plate_revision)?;
                State::check_plate(*index)?;
                (Some(&st.plate_overrides), SettingSource::Plate)
            }
            SettingsTarget::Object { object_id } => {
                if object_id != OBJECT_ID {
                    return Err(DriverError::StaleReference {
                        message: format!("unknown object {object_id}"),
                    });
                }
                (Some(&st.object_overrides), SettingSource::Object)
            }
        };
        let wanted: Vec<&str> = match keys {
            Some(keys) => keys.iter().map(String::as_str).collect(),
            None => st.settings.keys().map(String::as_str).collect(),
        };
        let mut out = Vec::with_capacity(wanted.len());
        for key in wanted {
            let Some(base) = st.settings.get(key) else {
                return Err(DriverError::InvalidParams {
                    message: format!("unknown setting {key}"),
                });
            };
            let (value, source) = match overrides.and_then(|o| o.get(key)) {
                Some(v) => (v.clone(), source),
                None => (base.clone(), SettingSource::Project),
            };
            out.push(EffectiveSetting {
                key: key.to_string(),
                value,
                source,
            });
        }
        Ok(out)
    }

    async fn settings_update(&self, values: &BTreeMap<String, String>) -> Result<(), DriverError> {
        let mut st = self.enter(DriverTool::SettingsUpdate).await?;
        check_keys(values, &WRITABLE_KEYS)?;
        st.take_snapshot();
        st.settings
            .extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
        Ok(())
    }

    async fn settings_override(
        &self,
        target: &SettingsTarget,
        values: &BTreeMap<String, String>,
    ) -> Result<PlateList, DriverError> {
        let mut st = self.enter(DriverTool::SettingsOverride).await?;
        match target {
            SettingsTarget::Project => {
                return Err(DriverError::InvalidParams {
                    message: "settings_override needs a plate or object scope".to_string(),
                });
            }
            SettingsTarget::Plate {
                index,
                plate_revision,
            } => {
                st.check_revision(plate_revision)?;
                State::check_plate(*index)?;
            }
            SettingsTarget::Object { object_id } => {
                if object_id != OBJECT_ID {
                    return Err(DriverError::StaleReference {
                        message: format!("unknown object {object_id}"),
                    });
                }
            }
        }
        check_keys(values, &OVERRIDE_KEYS)?;
        st.take_snapshot();
        let store = match target {
            SettingsTarget::Object { .. } => &mut st.object_overrides,
            _ => &mut st.plate_overrides,
        };
        store.extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
        Ok(st.plate_state())
    }

    async fn object_transform(
        &self,
        object_id: &str,
        instance_id: &str,
        t: &AbsoluteTransform,
    ) -> Result<ModelObject, DriverError> {
        let mut st = self.enter(DriverTool::ObjectTransform).await?;
        if object_id != OBJECT_ID {
            return Err(DriverError::StaleReference {
                message: format!("unknown object {object_id}"),
            });
        }
        if instance_id != INSTANCE_ID {
            return Err(DriverError::StaleReference {
                message: format!("unknown instance {instance_id}"),
            });
        }
        st.take_snapshot();
        st.instance.position_mm = t.position_mm;
        st.instance.rotation_deg = t.rotation_deg;
        st.instance.scale = t.scale;
        Ok(st.object())
    }

    async fn slice_start(
        &self,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<DriverJob, DriverError> {
        let mut st = self.enter(DriverTool::SliceStart).await?;
        st.check_revision(plate_revision)?;
        State::check_plate(plate_index)?;
        if st.slicing_running() {
            return Err(DriverError::Busy {
                message: "a slice job is already running".to_string(),
            });
        }
        st.job_counter += 1;
        let job = DriverJob {
            id: format!("mock-job-{}", st.job_counter),
            plate_index,
            status: JobStatus::Running,
            progress_percent: Some(0),
            message: None,
            error: None,
            slice_result_id: None,
            reused: false,
        };
        st.valid_result = None;
        st.jobs.insert(job.id.clone(), Job { job: job.clone() });
        Ok(job)
    }

    async fn job_get(&self, job_id: &str) -> Result<DriverJob, DriverError> {
        let mut st = self.enter(DriverTool::JobGet).await?;
        let st = &mut *st;
        let Some(entry) = st.jobs.get_mut(job_id) else {
            return Err(DriverError::StaleReference {
                message: format!("unknown job {job_id}"),
            });
        };
        let job = &mut entry.job;
        if !job.status.is_terminal() {
            let progress = job
                .progress_percent
                .unwrap_or(0)
                .saturating_add(50)
                .min(100);
            job.progress_percent = Some(progress);
            if progress >= 100 {
                let result_id = format!("mock-result-{}", job.id);
                job.status = JobStatus::Succeeded;
                job.slice_result_id = Some(result_id.clone());
                st.valid_result = Some((result_id, job.plate_index));
            }
        }
        Ok(job.clone())
    }

    async fn job_cancel(&self, job_id: &str) -> Result<DriverJob, DriverError> {
        let mut st = self.enter(DriverTool::JobCancel).await?;
        let Some(entry) = st.jobs.get_mut(job_id) else {
            return Err(DriverError::StaleReference {
                message: format!("unknown job {job_id}"),
            });
        };
        if !entry.job.status.is_terminal() {
            entry.job.status = JobStatus::Cancelled;
        }
        Ok(entry.job.clone())
    }

    async fn slice_state(&self) -> Result<SliceState, DriverError> {
        let st = self.enter(DriverTool::SliceGetState).await?;
        Ok(SliceState {
            running: st.slicing_running(),
            valid: st.valid_result.is_some(),
            estimated_time_s: st.valid_result.as_ref().and(st.estimated_time_s),
            slice_result_id: st.valid_result.as_ref().map(|(id, _)| id.clone()),
            plate_index: st.valid_result.as_ref().map(|(_, p)| *p),
        })
    }

    async fn preview_summary(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
    ) -> Result<SliceSummary, DriverError> {
        let st = self.enter(DriverTool::PreviewGetSummary).await?;
        st.check_revision(plate_revision)?;
        State::check_plate(plate_index)?;
        match &st.valid_result {
            Some((id, _)) if id == slice_result_id => Ok(SliceSummary {
                print_time_s: st.summary_stats.print_time_s,
                filaments: st.summary_stats.filaments.clone(),
                support_volume_mm3: st.summary_stats.support_volume_mm3,
                warnings: Vec::new(),
                raw: serde_json::json!({ "sliceResultId": slice_result_id }),
            }),
            _ => Err(DriverError::StaleReference {
                message: format!("slice result {slice_result_id} is not current"),
            }),
        }
    }
}

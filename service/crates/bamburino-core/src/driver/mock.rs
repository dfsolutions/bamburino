// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-memory [`DriverClient`] for tests (feature `mock`).
//!
//! Mirrors the observable behaviour of the real driver that the service
//! depends on: whitelists, a fresh `plateRevision` per `plate_list` that goes
//! stale on any edit, a modal-dialog gate on mutations, undo snapshots, and a
//! slice job that needs two polls to succeed, and `export_gcode` writing a
//! realistic Bambu header.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use super::map::{OVERRIDE_KEYS, WRITABLE_KEYS};
use super::{
    AbsoluteTransform, ActivePresets, AppState, DriverCapabilities, DriverClient, DriverError,
    DriverJob, DriverJobError, DriverTool, EffectiveSetting, FilamentUsage, Instance, JobStatus,
    ModelObject, Plate, PlateList, SettingSource, SettingsTarget, SliceState, SliceSummary,
};
use super::{
    AmsInventory, AmsUnit, DeviceSummary, ExportedPlate, FilamentAssignment, PlateContents,
    PlateDeleted,
};
use crate::build_info;
use crate::catalog::{Catalog, OptionKind};

/// Settings the mock reports but the driver does not accept for writing.
const READ_ONLY_KEYS: [&str; 1] = ["ironing_type"];
const OBJECT_ID: &str = "obj-1";
const INSTANCE_ID: &str = "inst-1";
/// Edge of the cube every mock object has, in millimetres.
const CUBE_MM: f64 = 20.0;
/// What `MockArrange::ShiftAndSwapPlates` moves every instance by.
const SWAP_SHIFT_MM: [f64; 3] = [10.0, 0.0, 0.0];
/// Magic bytes of the zip container every written 3MF starts with.
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";

/// One filament slot of the mock project (what the driver reads from the
/// project's `filament_settings_id`, `filament_type` and `filament_colour`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockFilament {
    pub preset: String,
    pub material: String,
    /// `None` is an empty segment in the serialized `filament_colour` list.
    pub colour: Option<String>,
}

/// What `ams_inventory` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockAms {
    /// The device answers but has no AMS: an inventory without units.
    Absent,
    /// The device is not reachable: `StaleReference`, like an unknown device.
    Offline,
    Units(Vec<AmsUnit>),
}

/// What the next arrange job does when it completes (second poll).
#[derive(Debug, Clone, PartialEq)]
pub enum MockArrange {
    /// Every instance moves by this vector.
    Shift([f64; 3]),
    /// Every instance moves by `[10, 0, 0]` and the contents of plates 0 and 1
    /// trade places (the arrange reshuffles plates, spec risk 1).
    ShiftAndSwapPlates,
    /// The job ends `Failed` with this message as a plain string error.
    Fail(String),
}

#[derive(Clone)]
struct MockObject {
    id: String,
    name: String,
    instances: Vec<Instance>,
    /// 1-based extruder, as `filament_assign` writes it.
    extruder: u32,
    /// True once `filament_assign` wrote the key (source `Object`).
    extruder_assigned: bool,
}

#[derive(Clone)]
struct MockPlate {
    /// Empty = unnamed: reported as `Plate <index + 1>`.
    name: String,
    locked: bool,
}

fn unnamed_plates(count: usize) -> Vec<MockPlate> {
    (0..count)
        .map(|_| MockPlate {
            name: String::new(),
            locked: false,
        })
        .collect()
}

#[derive(Clone)]
struct Snapshot {
    /// Logical time this snapshot was stored at (`m_current_time` in Studio).
    time: u64,
    settings: BTreeMap<String, String>,
    plate_overrides: BTreeMap<String, String>,
    object_overrides: BTreeMap<String, String>,
    objects: Vec<MockObject>,
    plates: Vec<MockPlate>,
}

#[derive(Default)]
struct SummaryStats {
    print_time_s: Option<f64>,
    filaments: Vec<FilamentUsage>,
    support_volume_mm3: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Slice,
    Arrange,
}

struct Job {
    job: DriverJob,
    kind: JobKind,
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
    fail_after: Option<(usize, DriverError)>,
    delay_next: Option<Duration>,
    // editor state
    settings: BTreeMap<String, String>,
    plate_overrides: BTreeMap<String, String>,
    object_overrides: BTreeMap<String, String>,
    objects: Vec<MockObject>,
    filaments: Vec<MockFilament>,
    dirty: bool,
    project_path: String,
    // undo
    snapshots: Vec<Snapshot>,
    /// Like `UndoRedo::StackImpl`: `current_time` only grows, `active_time`
    /// is what `undoSnapshot` reports and moves back on undo.
    current_time: u64,
    active_time: u64,
    undo_snapshot: Option<String>,
    undo_snapshot_supported: bool,
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
    // plates
    plates: Vec<MockPlate>,
    selected_plate: u32,
    // devices, arrange, files
    ams: MockAms,
    devices: Vec<DeviceSummary>,
    arrange: MockArrange,
    write_files: bool,
    assigned_indices: Vec<u32>,
    overwrite_flags: Vec<bool>,
    // export_gcode
    gcode_header: bool,
    exported: Vec<PathBuf>,
    export_counter: u64,
    /// Targeted faults: (tool, error, remaining times).
    tool_failures: Vec<(DriverTool, DriverError, u32)>,
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

fn cube_instance(id: &str, plate_index: u32) -> Instance {
    Instance {
        id: id.to_string(),
        index: 0,
        position_mm: [128.0, 128.0, 10.0],
        rotation_deg: [0.0; 3],
        scale: [1.0; 3],
        plate_index: plate_index as i32,
    }
}

fn cube_object(id: &str, name: &str, instance_id: &str, plate_index: u32) -> MockObject {
    MockObject {
        id: id.to_string(),
        name: name.to_string(),
        instances: vec![cube_instance(instance_id, plate_index)],
        extruder: 1,
        extruder_assigned: false,
    }
}

fn invalid(message: &str) -> DriverError {
    DriverError::InvalidParams {
        message: message.to_string(),
    }
}

fn unknown_object(object_id: &str) -> DriverError {
    DriverError::StaleReference {
        message: format!("unknown object {object_id}"),
    }
}

/// Re-indexes the instances of every object after an edit.
fn reindex(objects: &mut [MockObject]) {
    for object in objects {
        for (i, instance) in object.instances.iter_mut().enumerate() {
            instance.index = i as u32;
        }
    }
}

/// What both file writers put on disk: a zip header and a marker.
fn mock_3mf(what: &str) -> Vec<u8> {
    let mut bytes = ZIP_MAGIC.to_vec();
    bytes.extend_from_slice(format!("bamburino mock {what}").as_bytes());
    bytes
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), DriverError> {
    std::fs::write(path, bytes).map_err(|e| DriverError::Native {
        code: "NATIVE_ERROR".to_string(),
        message: e.to_string(),
    })
}

impl State {
    fn object_view(&self, index: usize) -> ModelObject {
        let object = &self.objects[index];
        let s = object.instances.first().map_or([1.0; 3], |i| i.scale);
        ModelObject {
            id: object.id.clone(),
            index: index as u32,
            name: object.name.clone(),
            dimensions_mm: [CUBE_MM * s[0], CUBE_MM * s[1], CUBE_MM * s[2]],
            instances: object.instances.clone(),
        }
    }

    fn object_position(&self, object_id: &str) -> Result<usize, DriverError> {
        self.objects
            .iter()
            .position(|o| o.id == object_id)
            .ok_or_else(|| unknown_object(object_id))
    }

    fn plate_count(&self) -> u32 {
        self.plates.len() as u32
    }

    /// `(object position, instance position)` of every instance on a plate.
    fn instances_on(&self, plate: u32) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (o, object) in self.objects.iter().enumerate() {
            for (i, instance) in object.instances.iter().enumerate() {
                if instance.plate_index == plate as i32 {
                    out.push((o, i));
                }
            }
        }
        out
    }

    /// A test-setup edit of the scene: stales every issued plate revision and
    /// invalidates the slice result, like any edit, without an undo step.
    fn scene_changed(&mut self) {
        self.epoch += 1;
        self.invalidate_slice();
    }

    fn shift_instances(&mut self, by: [f64; 3]) {
        for object in &mut self.objects {
            for instance in &mut object.instances {
                for (axis, delta) in by.iter().enumerate() {
                    instance.position_mm[axis] += delta;
                }
            }
        }
    }

    fn plate_is_locked(&self, plate: i32) -> bool {
        usize::try_from(plate)
            .ok()
            .and_then(|p| self.plates.get(p))
            .is_some_and(|p| p.locked)
    }

    fn colour_list(&self) -> String {
        self.filaments
            .iter()
            .map(|f| f.colour.clone().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(";")
    }

    fn type_list(&self) -> String {
        self.filaments
            .iter()
            .map(|f| f.material.as_str())
            .collect::<Vec<_>>()
            .join(";")
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
            plates: self
                .plates
                .iter()
                .enumerate()
                .map(|(index, plate)| Plate {
                    index: index as u32,
                    name: if plate.name.is_empty() {
                        format!("Plate {}", index + 1)
                    } else {
                        plate.name.clone()
                    },
                    locked: plate.locked,
                })
                .collect(),
            selected_index: self.selected_plate,
            plate_revision: revision,
        }
    }

    /// Records an undo snapshot of the state before a mutation and
    /// invalidates every issued plate revision.
    fn take_snapshot(&mut self) {
        self.snapshots.push(Snapshot {
            time: self.current_time,
            settings: self.settings.clone(),
            plate_overrides: self.plate_overrides.clone(),
            object_overrides: self.object_overrides.clone(),
            objects: self.objects.clone(),
            plates: self.plates.clone(),
        });
        // `m_active_snapshot_time = ++m_current_time`: after an undo the new
        // snapshot jumps over the undone ones.
        self.current_time += 1;
        self.active_time = self.current_time;
        self.undo_snapshot = Some(self.active_time.to_string());
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

    fn check_plate(&self, index: u32) -> Result<(), DriverError> {
        if index < self.plate_count() {
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
            .chain(READ_ONLY_KEYS.iter())
            .map(|k| (k.to_string(), default_string(k)))
            .collect();
        Self {
            state: Mutex::new(State {
                calls: Vec::new(),
                app_version: build_info::STUDIO_VERSION.to_string(),
                patch_id: build_info::EXPECTED_PATCH_ID.map(str::to_string),
                session_id: "mock-session".to_string(),
                unreachable: false,
                undo_snapshot_supported: true,
                modal_open: false,
                fail_next: None,
                fail_after: None,
                delay_next: None,
                settings,
                plate_overrides: BTreeMap::new(),
                object_overrides: BTreeMap::new(),
                objects: vec![cube_object(OBJECT_ID, "cube", INSTANCE_ID, 0)],
                filaments: vec![MockFilament {
                    preset: "Mock PLA".to_string(),
                    material: "PLA".to_string(),
                    colour: Some("#00AE42".to_string()),
                }],
                dirty: false,
                project_path: String::new(),
                snapshots: Vec::new(),
                current_time: 0,
                active_time: 0,
                undo_snapshot: None,
                epoch: 0,
                revision_counter: 0,
                issued: BTreeMap::new(),
                job_counter: 0,
                jobs: BTreeMap::new(),
                valid_result: None,
                summary_stats: SummaryStats::default(),
                estimated_time_s: Some(1800.0),
                plates: unnamed_plates(1),
                selected_plate: 0,
                ams: MockAms::Absent,
                devices: Vec::new(),
                arrange: MockArrange::Shift([10.0, 0.0, 0.0]),
                write_files: true,
                assigned_indices: Vec::new(),
                overwrite_flags: Vec::new(),
                gcode_header: true,
                exported: Vec::new(),
                export_counter: 0,
                tool_failures: Vec::new(),
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
    /// moves `undo_snapshot` back to its time (a later edit jumps over it).
    pub fn user_undo(&self) {
        let mut st = self.lock();
        if let Some(snap) = st.snapshots.pop() {
            st.settings = snap.settings;
            st.plate_overrides = snap.plate_overrides;
            st.object_overrides = snap.object_overrides;
            st.objects = snap.objects;
            st.plates = snap.plates;
            st.selected_plate = st.selected_plate.min(st.plate_count() - 1);
            // Undo moves the active time back to the restored snapshot; the
            // current time is not decremented.
            st.active_time = snap.time;
            st.undo_snapshot = Some(snap.time.to_string());
        }
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

    /// Number of plates the mock reports (default 1). Plate 0 is selected.
    /// Resizes the plate list; new plates are unnamed and unlocked, instances
    /// on removed plates move to the last one. A setup edit: it stales plate
    /// revisions and the slice result, so read revisions AFTER it.
    pub fn set_plate_count(&self, count: u32) {
        let mut st = self.lock();
        st.scene_changed();
        st.plates.resize(
            count.max(1) as usize,
            MockPlate {
                name: String::new(),
                locked: false,
            },
        );
        let last = st.plates.len() as i32 - 1;
        for object in &mut st.objects {
            for instance in &mut object.instances {
                instance.plate_index = instance.plate_index.min(last);
            }
        }
        st.selected_plate = st.selected_plate.min(st.plate_count() - 1);
    }

    /// Adds a 20 mm cube with one instance `inst-<object_id>` (extruder 1) on
    /// `plate_index`. Missing plates are created. A setup edit: no undo step,
    /// but it stales plate revisions and the slice result (it changes what
    /// would be sliced), so read revisions AFTER it.
    pub fn add_object(&self, object_id: &str, name: &str, plate_index: u32) {
        let mut st = self.lock();
        st.scene_changed();
        let needed = plate_index as usize + 1;
        if st.plates.len() < needed {
            let missing = needed - st.plates.len();
            st.plates.extend(unnamed_plates(missing));
        }
        st.objects.push(cube_object(
            object_id,
            name,
            &format!("inst-{object_id}"),
            plate_index,
        ));
    }

    /// Adds another instance to an existing object on `plate_index`. Same
    /// setup-edit semantics as [`Self::add_object`]: read revisions AFTER it.
    ///
    /// # Panics
    /// If `object_id` does not exist.
    pub fn add_instance(&self, object_id: &str, instance_id: &str, plate_index: u32) {
        let mut st = self.lock();
        st.scene_changed();
        let needed = plate_index as usize + 1;
        if st.plates.len() < needed {
            let missing = needed - st.plates.len();
            st.plates.extend(unnamed_plates(missing));
        }
        let pos = st.object_position(object_id).expect("mock object exists");
        st.objects[pos]
            .instances
            .push(cube_instance(instance_id, plate_index));
        reindex(&mut st.objects);
    }

    /// Writes the raw rotation of an instance (may hold `-0.0`). Setup edit,
    /// same semantics as [`Self::add_object`]: read revisions AFTER it.
    ///
    /// # Panics
    /// If the object or the instance does not exist.
    pub fn set_instance_rotation(&self, object_id: &str, instance_id: &str, deg: [f64; 3]) {
        let mut st = self.lock();
        st.scene_changed();
        let pos = st.object_position(object_id).expect("mock object exists");
        let instance = st.objects[pos]
            .instances
            .iter_mut()
            .find(|i| i.id == instance_id)
            .expect("mock instance exists");
        instance.rotation_deg = deg;
    }

    /// Replaces the project's filament slots (default: one "Mock PLA"). Setup
    /// edit: stales plate revisions and the slice result; read revisions AFTER.
    pub fn set_filaments(&self, f: Vec<MockFilament>) {
        let mut st = self.lock();
        st.scene_changed();
        st.filaments = f;
    }

    /// Simulates the user locking a plate. No driver tool does it. Like the
    /// driver's `plate.lock` it takes an undo snapshot: the project revision
    /// moves, plate revisions go stale and the slice result is invalidated.
    pub fn lock_plate(&self, index: u32, locked: bool) {
        let mut st = self.lock();
        if (index as usize) < st.plates.len() {
            st.take_snapshot();
            st.plates[index as usize].locked = locked;
        }
    }

    /// What `ams_inventory` answers (default `Absent`). Setup edit: stales
    /// plate revisions and the slice result; read revisions AFTER it.
    pub fn set_ams(&self, ams: MockAms) {
        let mut st = self.lock();
        st.scene_changed();
        st.ams = ams;
    }

    /// What `device_list` answers (default none). Setup edit, same semantics
    /// as [`Self::set_ams`].
    pub fn set_devices(&self, d: Vec<DeviceSummary>) {
        let mut st = self.lock();
        st.scene_changed();
        st.devices = d;
    }

    /// What the next arrange jobs do (default `Shift([10, 0, 0])`).
    pub fn set_arrange(&self, a: MockArrange) {
        self.lock().arrange = a;
    }

    /// False: `project_save` and `export_sliced_3mf` succeed but write nothing.
    pub fn set_write_files(&self, on: bool) {
        self.lock().write_files = on;
    }

    /// `(object_id, instance_id)` of every instance on a plate.
    pub fn instances_on_plate(&self, index: u32) -> Vec<(String, String)> {
        let st = self.lock();
        st.instances_on(index)
            .into_iter()
            .map(|(o, i)| {
                (
                    st.objects[o].id.clone(),
                    st.objects[o].instances[i].id.clone(),
                )
            })
            .collect()
    }

    /// 1-based extruder of an object, what `filament_assign` wrote.
    ///
    /// # Panics
    /// If the object does not exist.
    pub fn extruder_of(&self, object_id: &str) -> u32 {
        let st = self.lock();
        let pos = st.object_position(object_id).expect("mock object exists");
        st.objects[pos].extruder
    }

    /// Every (accepted) `filamentIndex` `filament_assign` received, 0-based.
    pub fn assigned_filament_indices(&self) -> Vec<u32> {
        self.lock().assigned_indices.clone()
    }

    /// The `overwrite` value of every `project_save` / `export_sliced_3mf`
    /// received, refused calls included.
    pub fn file_overwrite_flags(&self) -> Vec<bool> {
        self.lock().overwrite_flags.clone()
    }

    /// Whether the exported G-code carries the Bambu header with the filament
    /// totals (default true; it is only written when filaments are set).
    pub fn set_gcode_header(&self, present: bool) {
        self.lock().gcode_header = present;
    }

    /// Paths `export_gcode` was asked to write, in order.
    pub fn exported_paths(&self) -> Vec<PathBuf> {
        self.lock().exported.clone()
    }

    /// The next `times` calls of `tool` fail with `err` (after the usual
    /// prologue, so they are recorded in `calls`).
    pub fn fail_tool(&self, tool: DriverTool, err: DriverError, times: u32) {
        self.lock().tool_failures.push((tool, err, times));
    }

    /// Whether `app_get_state` reports `undoSnapshot` (plan A patch 0120).
    /// Default true; false simulates a build without the patch.
    pub fn set_undo_snapshot_supported(&self, supported: bool) {
        self.lock().undo_snapshot_supported = supported;
    }

    /// Simulates the user adding a per-object override in the GUI.
    pub fn user_object_override(&self, key: &str, value: &str) {
        let mut st = self.lock();
        st.take_snapshot();
        st.object_overrides
            .insert(key.to_string(), value.to_string());
    }

    /// Simulates the user adding a per-plate override in the GUI.
    pub fn user_plate_override(&self, key: &str, value: &str) {
        let mut st = self.lock();
        st.take_snapshot();
        st.plate_overrides
            .insert(key.to_string(), value.to_string());
    }

    pub fn set_modal(&self, open: bool) {
        self.lock().modal_open = open;
    }

    /// The next call (of any kind) fails with `err`.
    pub fn fail_next(&self, err: DriverError) {
        self.lock().fail_next = Some(err);
    }

    /// The call after the next `skip` calls fails with `err`.
    pub fn fail_after(&self, skip: usize, err: DriverError) {
        self.lock().fail_after = Some((skip, err));
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
            let mut fail = st.fail_next.take();
            if fail.is_none() {
                match st.fail_after.take() {
                    Some((0, err)) => fail = Some(err),
                    Some((n, err)) => st.fail_after = Some((n - 1, err)),
                    None => {}
                }
            }
            (st.delay_next.take(), fail)
        };
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        if let Some(err) = fail {
            return Err(err);
        }
        let mut st = self.lock();
        if let Some(pos) = st
            .tool_failures
            .iter()
            .position(|(t, _, n)| *t == tool && *n > 0)
        {
            st.tool_failures[pos].2 -= 1;
            return Err(st.tool_failures[pos].1.clone());
        }
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
            project_name: Path::new(&st.project_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled")
                .to_string(),
            project_path: st.project_path.clone(),
            dirty: st.dirty,
            presets_dirty: st.dirty,
            ui_job_running: false,
            slicing_running: st.slicing_running(),
            modal_open: st.modal_open,
            object_count: st.objects.len() as u32,
            plate_count: st.plate_count(),
            selected_plate_index: st.selected_plate,
            undo_snapshot: st
                .undo_snapshot_supported
                .then(|| st.undo_snapshot.clone())
                .flatten(),
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
        st.check_plate(plate_index)?;
        // Like the driver, selecting a plate takes an undo snapshot and so
        // invalidates every previously issued plate revision.
        st.take_snapshot();
        st.selected_plate = plate_index;
        Ok(st.plate_state())
    }

    async fn model_list(&self) -> Result<Vec<ModelObject>, DriverError> {
        let st = self.enter(DriverTool::ModelList).await?;
        Ok((0..st.objects.len()).map(|i| st.object_view(i)).collect())
    }

    async fn active_presets(&self) -> Result<ActivePresets, DriverError> {
        // The driver needs one `preset_list` call per kind.
        let mut st = self.enter(DriverTool::PresetList).await?;
        st.calls.push(DriverTool::PresetList.name().to_string());
        st.calls.push(DriverTool::PresetList.name().to_string());
        Ok(ActivePresets {
            printer: Some("Mock Printer".to_string()),
            process: Some("0.20mm Standard".to_string()),
            filaments: st.filaments.iter().map(|f| f.preset.clone()).collect(),
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
                st.check_plate(*index)?;
                (Some(&st.plate_overrides), SettingSource::Plate)
            }
            SettingsTarget::Object { object_id } => {
                st.object_position(object_id)?;
                (Some(&st.object_overrides), SettingSource::Object)
            }
        };
        let mut wanted: Vec<&str> = match keys {
            Some(keys) => keys.iter().map(String::as_str).collect(),
            None => st.settings.keys().map(String::as_str).collect(),
        };
        // The revision fingerprint reads every key of an object, so the
        // object's slot is part of the unfiltered listing.
        if keys.is_none() && matches!(target, SettingsTarget::Object { .. }) {
            wanted.push("extruder");
        }
        let mut out = Vec::with_capacity(wanted.len());
        for key in wanted {
            // Keys the driver serves straight from the model / project config.
            let derived = match (target, key) {
                (SettingsTarget::Object { object_id }, "extruder") => {
                    let object = &st.objects[st.object_position(object_id)?];
                    let source = if object.extruder_assigned {
                        SettingSource::Object
                    } else {
                        SettingSource::Project
                    };
                    Some((object.extruder.to_string(), source))
                }
                (SettingsTarget::Project, "filament_colour") => {
                    Some((st.colour_list(), SettingSource::Project))
                }
                (SettingsTarget::Project, "filament_type") => {
                    Some((st.type_list(), SettingSource::Project))
                }
                _ => None,
            };
            if let Some((value, source)) = derived {
                out.push(EffectiveSetting {
                    key: key.to_string(),
                    value,
                    source,
                });
                continue;
            }
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
                st.check_plate(*index)?;
            }
            SettingsTarget::Object { object_id } => {
                st.object_position(object_id)?;
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
        let o = st.object_position(object_id)?;
        let Some(i) = st.objects[o]
            .instances
            .iter()
            .position(|i| i.id == instance_id)
        else {
            return Err(DriverError::StaleReference {
                message: format!("unknown instance {instance_id}"),
            });
        };
        st.take_snapshot();
        let instance = &mut st.objects[o].instances[i];
        instance.position_mm = t.position_mm;
        instance.rotation_deg = t.rotation_deg;
        instance.scale = t.scale;
        Ok(st.object_view(o))
    }

    async fn slice_start(
        &self,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<DriverJob, DriverError> {
        let mut st = self.enter(DriverTool::SliceStart).await?;
        st.check_revision(plate_revision)?;
        st.check_plate(plate_index)?;
        if plate_index != st.selected_plate {
            return Err(DriverError::InvalidParams {
                message: "Only the selected plate is supported; select it explicitly first"
                    .to_string(),
            });
        }
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
        st.jobs.insert(
            job.id.clone(),
            Job {
                job: job.clone(),
                kind: JobKind::Slice,
            },
        );
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
        let kind = entry.kind;
        let job = &mut entry.job;
        let mut finished_arrange = false;
        if !job.status.is_terminal() {
            let progress = job
                .progress_percent
                .unwrap_or(0)
                .saturating_add(50)
                .min(100);
            job.progress_percent = Some(progress);
            if progress >= 100 {
                match kind {
                    JobKind::Slice => {
                        let result_id = format!("mock-result-{}", job.id);
                        job.status = JobStatus::Succeeded;
                        job.slice_result_id = Some(result_id.clone());
                        st.valid_result = Some((result_id, job.plate_index));
                    }
                    JobKind::Arrange => finished_arrange = true,
                }
            }
        }
        let mut out = job.clone();
        if finished_arrange {
            // An arrange job never produces a slice result.
            st.valid_result = None;
            match st.arrange.clone() {
                MockArrange::Fail(message) => {
                    out.status = JobStatus::Failed;
                    out.error = Some(DriverJobError {
                        code: "NATIVE_ERROR".to_string(),
                        message,
                    });
                }
                MockArrange::Shift(by) => {
                    st.shift_instances(by);
                    out.status = JobStatus::Succeeded;
                }
                MockArrange::ShiftAndSwapPlates => {
                    st.shift_instances(SWAP_SHIFT_MM);
                    for object in &mut st.objects {
                        for instance in &mut object.instances {
                            instance.plate_index = match instance.plate_index {
                                0 if st.plates.len() > 1 => 1,
                                1 => 0,
                                other => other,
                            };
                        }
                    }
                    out.status = JobStatus::Succeeded;
                }
            }
            if let Some(entry) = st.jobs.get_mut(job_id) {
                entry.job = out.clone();
            }
        }
        Ok(out)
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
        st.check_plate(plate_index)?;
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

    async fn export_gcode(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
        path: &Path,
    ) -> Result<DriverJob, DriverError> {
        let mut st = self.enter(DriverTool::ExportGcode).await?;
        st.check_revision(plate_revision)?;
        st.check_plate(plate_index)?;
        match &st.valid_result {
            Some((id, _)) if id == slice_result_id => {}
            _ => {
                return Err(DriverError::StaleReference {
                    message: format!("slice result {slice_result_id} is not current"),
                });
            }
        }
        let invalid = |m: &str| DriverError::InvalidParams {
            message: m.to_string(),
        };
        if !path.is_absolute() || path.extension().and_then(|e| e.to_str()) != Some("gcode") {
            return Err(invalid("Export requires an absolute .gcode path"));
        }
        if !path.parent().is_some_and(Path::is_dir) {
            return Err(invalid("Export parent directory does not exist"));
        }
        if path.exists() {
            return Err(invalid("Destination exists; explicitly set overwrite=true"));
        }
        let text = gcode_text(&st.summary_stats.filaments, st.gcode_header);
        std::fs::write(path, text).map_err(|e| DriverError::Native {
            code: "NATIVE_ERROR".to_string(),
            message: e.to_string(),
        })?;
        st.exported.push(path.to_path_buf());
        st.export_counter += 1;
        Ok(DriverJob {
            id: format!("mock-export-{}", st.export_counter),
            plate_index,
            status: JobStatus::Succeeded,
            progress_percent: Some(100),
            message: None,
            error: None,
            slice_result_id: Some(slice_result_id.to_string()),
            reused: false,
        })
    }

    async fn plate_create(&self) -> Result<PlateList, DriverError> {
        let mut st = self.enter(DriverTool::PlateCreate).await?;
        st.take_snapshot();
        st.plates.extend(unnamed_plates(1));
        Ok(st.plate_state())
    }

    async fn plate_rename(
        &self,
        plate_index: u32,
        plate_revision: &str,
        name: &str,
    ) -> Result<PlateList, DriverError> {
        let mut st = self.enter(DriverTool::PlateRename).await?;
        st.check_revision(plate_revision)?;
        st.check_plate(plate_index)?;
        st.take_snapshot();
        st.plates[plate_index as usize].name = name.to_string();
        Ok(st.plate_state())
    }

    async fn plate_delete(
        &self,
        plate_index: u32,
        plate_revision: &str,
        contents: PlateContents,
        target_plate_index: Option<u32>,
    ) -> Result<PlateDeleted, DriverError> {
        let mut st = self.enter(DriverTool::PlateDelete).await?;
        st.check_revision(plate_revision)?;
        st.check_plate(plate_index)?;
        if st.plates.len() <= 1 {
            return Err(invalid("Cannot delete the last plate"));
        }
        if st.plates[plate_index as usize].locked {
            return Err(invalid("Unlock the plate before deleting it"));
        }
        let on_plate = st.instances_on(plate_index);
        if contents == PlateContents::RequireEmpty && !on_plate.is_empty() {
            return Err(invalid(
                "Plate contains instances; choose contents=move or contents=delete explicitly",
            ));
        }
        let mut target = None;
        if contents == PlateContents::Move {
            let Some(t) = target_plate_index else {
                return Err(invalid("targetPlateIndex is required when contents=move"));
            };
            st.check_plate(t)?;
            if t == plate_index {
                return Err(invalid("Move destination must be another plate"));
            }
            if st.plates[t as usize].locked || !st.instances_on(t).is_empty() {
                return Err(invalid("Move destination must be unlocked and empty"));
            }
            target = Some(t);
        }
        let doomed: BTreeSet<usize> = on_plate.iter().map(|(o, _)| *o).collect();
        if contents == PlateContents::Delete {
            let shared = doomed.iter().any(|o| {
                st.objects[*o]
                    .instances
                    .iter()
                    .any(|i| i.plate_index != plate_index as i32)
            });
            if shared {
                return Err(invalid(
                    "An object also has instances outside this plate; move its contents instead",
                ));
            }
        }
        st.take_snapshot();
        let mut moved = 0;
        let mut deleted = 0;
        match (contents, target) {
            (PlateContents::Move, Some(t)) => {
                for (o, i) in &on_plate {
                    st.objects[*o].instances[*i].plate_index = t as i32;
                }
                moved = on_plate.len() as u32;
            }
            (PlateContents::Delete, _) => {
                let mut position = 0;
                st.objects.retain(|_| {
                    let keep = !doomed.contains(&position);
                    position += 1;
                    keep
                });
                reindex(&mut st.objects);
                deleted = doomed.len() as u32;
            }
            _ => {}
        }
        st.plates.remove(plate_index as usize);
        for object in &mut st.objects {
            for instance in &mut object.instances {
                if instance.plate_index > plate_index as i32 {
                    instance.plate_index -= 1;
                }
            }
        }
        st.selected_plate = match st.selected_plate {
            s if s > plate_index => s - 1,
            s => s.min(st.plate_count() - 1),
        };
        let destination_plate_index = target.map(|t| if t > plate_index { t - 1 } else { t });
        Ok(PlateDeleted {
            plates: st.plate_state(),
            moved_instances: moved,
            deleted_objects: deleted,
            destination_plate_index,
        })
    }

    async fn plate_move_instance(
        &self,
        object_id: &str,
        instance_id: &str,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<PlateList, DriverError> {
        let mut st = self.enter(DriverTool::PlateMoveInstance).await?;
        st.check_revision(plate_revision)?;
        let o = st.object_position(object_id)?;
        let Some(i) = st.objects[o]
            .instances
            .iter()
            .position(|i| i.id == instance_id)
        else {
            return Err(DriverError::StaleReference {
                message: format!("unknown instance {instance_id}"),
            });
        };
        st.check_plate(plate_index)?;
        let source = st.objects[o].instances[i].plate_index;
        if source < 0 {
            return Err(invalid("Instance is not assigned to a plate"));
        }
        if st.plate_is_locked(source) || st.plates[plate_index as usize].locked {
            return Err(invalid("Unlock both plates before moving the instance"));
        }
        if source != plate_index as i32 {
            st.take_snapshot();
            st.objects[o].instances[i].plate_index = plate_index as i32;
        }
        Ok(st.plate_state())
    }

    async fn filament_assign(
        &self,
        object_id: &str,
        filament_index: u32,
    ) -> Result<FilamentAssignment, DriverError> {
        let mut st = self.enter(DriverTool::FilamentAssign).await?;
        let o = st.object_position(object_id)?;
        if st.objects[o]
            .instances
            .iter()
            .any(|i| st.plate_is_locked(i.plate_index))
        {
            return Err(invalid("Unlock every plate containing the object first"));
        }
        let Some(slot) = st.filaments.get(filament_index as usize) else {
            return Err(invalid("Index is outside current state; list again"));
        };
        let preset = slot.preset.clone();
        st.take_snapshot();
        st.objects[o].extruder = filament_index + 1;
        st.objects[o].extruder_assigned = true;
        st.assigned_indices.push(filament_index);
        Ok(FilamentAssignment {
            object_id: object_id.to_string(),
            filament_index,
            preset,
        })
    }

    async fn arrange_start(&self) -> Result<DriverJob, DriverError> {
        let mut st = self.enter(DriverTool::ArrangeStart).await?;
        st.take_snapshot();
        st.job_counter += 1;
        let job = DriverJob {
            id: format!("mock-job-{}:arrange", st.job_counter),
            plate_index: st.selected_plate,
            status: JobStatus::Running,
            progress_percent: Some(0),
            message: None,
            error: None,
            slice_result_id: None,
            reused: false,
        };
        st.jobs.insert(
            job.id.clone(),
            Job {
                job: job.clone(),
                kind: JobKind::Arrange,
            },
        );
        Ok(job)
    }

    async fn device_list(&self) -> Result<Vec<DeviceSummary>, DriverError> {
        let st = self.enter(DriverTool::DeviceList).await?;
        Ok(st.devices.clone())
    }

    async fn ams_inventory(&self, device_id: &str) -> Result<AmsInventory, DriverError> {
        let st = self.enter(DriverTool::AmsInventory).await?;
        match &st.ams {
            MockAms::Absent => Ok(AmsInventory { units: Vec::new() }),
            MockAms::Offline => Err(DriverError::StaleReference {
                message: format!("unknown device {device_id}"),
            }),
            MockAms::Units(units) => Ok(AmsInventory {
                units: units.clone(),
            }),
        }
    }

    async fn project_save(&self, path: &Path, overwrite: bool) -> Result<(), DriverError> {
        let mut st = self.enter(DriverTool::ProjectSave).await?;
        st.overwrite_flags.push(overwrite);
        if !path.is_absolute() || path.extension().and_then(|e| e.to_str()) != Some("3mf") {
            return Err(invalid("Save requires an absolute .3mf destination"));
        }
        if !path.parent().is_some_and(Path::is_dir) {
            return Err(invalid("Destination directory does not exist"));
        }
        if path.exists() && !overwrite {
            return Err(invalid("Destination exists; set overwrite=true"));
        }
        if st.write_files {
            write_file(path, &mock_3mf("project"))?;
        }
        // No undo snapshot: only the project path and the dirty flag change.
        st.project_path = path.to_string_lossy().into_owned();
        st.dirty = false;
        Ok(())
    }

    async fn export_sliced_3mf(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
        path: &Path,
        overwrite: bool,
    ) -> Result<ExportedPlate, DriverError> {
        let mut st = self.enter(DriverTool::ExportSliced3mf).await?;
        st.overwrite_flags.push(overwrite);
        if !path.is_absolute()
            || !path
                .to_string_lossy()
                .to_lowercase()
                .ends_with(".gcode.3mf")
        {
            return Err(invalid(
                "Sliced export requires an absolute .gcode.3mf path",
            ));
        }
        if !path.parent().is_some_and(Path::is_dir) {
            return Err(invalid("Export parent directory does not exist"));
        }
        if path.exists() && !path.is_file() {
            return Err(invalid("Destination must be a regular file"));
        }
        if path.exists() && !overwrite {
            return Err(invalid("Destination exists; explicitly set overwrite=true"));
        }
        st.check_revision(plate_revision)?;
        st.check_plate(plate_index)?;
        if plate_index != st.selected_plate {
            return Err(invalid(
                "Only the selected plate is supported; select it explicitly first",
            ));
        }
        match &st.valid_result {
            Some((id, plate)) if id == slice_result_id && *plate == st.selected_plate => {}
            _ => {
                return Err(DriverError::StaleReference {
                    message: "Selected plate no longer has the requested completed slice"
                        .to_string(),
                });
            }
        }
        let bytes = mock_3mf(&format!("sliced plate {plate_index} {slice_result_id}"));
        if st.write_files {
            write_file(path, &bytes)?;
        }
        Ok(ExportedPlate {
            slice_result_id: slice_result_id.to_string(),
            plate_index,
            path: path.to_string_lossy().into_owned(),
            bytes: bytes.len() as u64,
        })
    }
}

/// A realistic Bambu G-code head. Like the real one it lists ONLY the used
/// filaments (those with grams > 0), in id order, and names them in
/// `; filament: <1-based ids>`; the header is written only when `header` and
/// at least one filament is used.
fn gcode_text(filaments: &[FilamentUsage], header: bool) -> String {
    let mut out = String::from("; HEADER_BLOCK_START\n; BambuStudio 02.05.00.66\n");
    let mut used: Vec<&FilamentUsage> = filaments
        .iter()
        .filter(|u| u.grams.unwrap_or(0.0) > 0.0)
        .collect();
    used.sort_by_key(|u| u.index);
    if header && !used.is_empty() {
        let join = |f: &dyn Fn(&FilamentUsage) -> f64| {
            used.iter()
                .map(|u| format!("{:.2}", f(u)))
                .collect::<Vec<_>>()
                .join(",")
        };
        let mm = join(&|u| u.meters.unwrap_or(0.0) * 1000.0);
        let g = join(&|u| u.grams.unwrap_or(0.0));
        // PLA density 1.24 g/cm3.
        let cm3 = join(&|u| u.grams.unwrap_or(0.0) / 1.24);
        let ids = used
            .iter()
            .map(|u| (u.index + 1).to_string())
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&format!(
            "; total filament length [mm] : {mm}\n; total filament volume [cm^3] : {cm3}\n; total filament weight [g] : {g}\n; filament: {ids}\n"
        ));
    }
    out.push_str("; HEADER_BLOCK_END\nG1 X10 Y10 E1\n");
    out
}

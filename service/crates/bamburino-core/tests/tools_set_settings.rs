// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::driver::{
    AbsoluteTransform, ActivePresets, AppState, DriverCapabilities, DriverClient, DriverError,
    DriverJob, EffectiveSetting, ModelObject, PlateList, SettingsTarget, SliceState, SliceSummary,
};
use bamburino_core::errors::ToolErrorCode;
use bamburino_core::tools::{Change, SetSettingsArgs, TargetRef, ToolConfig, Tools};
use bamburino_core::validate::Scope;
use serde_json::{Value, json};

fn tools_over(driver: Arc<dyn DriverClient>) -> Tools {
    Tools::new(
        driver,
        Arc::new(FixedProcessProbe(true)),
        ToolConfig {
            allow_version_mismatch: false,
            poll_every: Duration::from_secs(60),
            idle_after: Duration::from_secs(60),
            token_present: true,
        },
        std::env::temp_dir().join("bamburino-tests-unused"),
    )
}

fn tools_with(mock: &Arc<MockDriver>) -> Tools {
    tools_over(mock.clone())
}

async fn revision(t: &Tools) -> u64 {
    t.studio_get_state().await.unwrap()["revision"]
        .as_u64()
        .unwrap()
}

fn change(key: &str, value: Value) -> Change {
    Change {
        key: key.to_string(),
        value,
        scope: None,
        target: None,
    }
}

fn args(changes: Vec<Change>, expected_revision: u64, dry_run: Option<bool>) -> SetSettingsArgs {
    SetSettingsArgs {
        changes,
        expected_revision,
        dry_run,
    }
}

fn demo() -> Vec<Change> {
    vec![
        change("sparse_infill_density", json!(35)),
        change("sparse_infill_pattern", json!("gyroid")),
        change("wall_loops", json!(4)),
        change("layer_height", json!(0.2)),
    ]
}

fn plate_change(key: &str, value: Value, plate_id: u32) -> Change {
    Change {
        scope: Some(Scope::Plate),
        target: Some(TargetRef {
            plate_id: Some(plate_id),
            object_id: None,
        }),
        ..change(key, value)
    }
}

fn object_change(key: &str, value: Value, object_id: &str) -> Change {
    Change {
        scope: Some(Scope::Object),
        target: Some(TargetRef {
            plate_id: None,
            object_id: Some(object_id.to_string()),
        }),
        ..change(key, value)
    }
}

fn count(mock: &MockDriver, name: &str) -> usize {
    mock.calls().iter().filter(|c| *c == name).count()
}

fn writes(mock: &MockDriver) -> usize {
    count(mock, "settings_update") + count(mock, "settings_override")
}

#[tokio::test]
async fn demo_batch_applies_four_values() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("top_shell_layers", "3"); // undo_snapshot is Some: the check runs
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let v = t
        .studio_set_settings(args(demo(), rev, None))
        .await
        .unwrap();
    let applied: Vec<&str> = v["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["applied"].as_str().unwrap())
        .collect();
    assert_eq!(applied, ["35%", "gyroid", "4", "0.2"]);
    assert_eq!(v["applied"][0]["key"], "sparse_infill_density");
    assert_eq!(v["applied"][0]["scope"], "project");
    assert_eq!(v["applied"][0]["requested"], json!(35));
    assert_eq!(v["dry_run"], false);
    let new_rev = v["revision"].as_u64().unwrap();
    assert!(new_rev > rev);
    assert_eq!(v["diff"].as_array().unwrap().len(), 4);
    assert_eq!(v["diff"][0]["to"], "35%");
    assert_eq!(count(&mock, "settings_update"), 1);
    // The new revision is the live one: a follow-up call with it is accepted.
    assert_eq!(revision(&t).await, new_rev);
    t.studio_set_settings(args(vec![change("wall_loops", json!(5))], new_rev, None))
        .await
        .unwrap();
}

#[tokio::test]
async fn stale_revision_returns_409_with_state() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    mock.user_edit("sparse_infill_density", "20%");
    let e = t
        .studio_set_settings(args(demo(), rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(e.http_status, 409);
    assert_eq!(e.details["expected"], rev);
    let current = e.details["current"].as_u64().unwrap();
    assert!(current > rev);
    assert_eq!(e.details["state"]["revision"].as_u64().unwrap(), current);
    assert_eq!(writes(&mock), 0);
    t.studio_set_settings(args(demo(), current, None))
        .await
        .unwrap();
    assert_eq!(count(&mock, "settings_update"), 1);
}

#[tokio::test]
async fn non_writable_key_rejects_whole_batch() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![
                change("sparse_infill_density", json!(35)),
                change("ironing_type", json!("top")),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::NotWritableByDriver);
    assert_eq!(e.http_status, 422);
    assert_eq!(e.details["keys"], json!(["ironing_type"]));
    assert_eq!(e.details["errors"][0]["key"], "ironing_type");
    assert_eq!(writes(&mock), 0);
}

#[tokio::test]
async fn invalid_value_rejects_whole_batch() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![
                change("sparse_infill_density", json!(35)),
                change("wall_loops", json!("many")),
                change("no_such_key", json!(1)),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    let errors = e.details["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["key"], "wall_loops");
    assert_eq!(errors[0]["code"], "INVALID_VALUE");
    assert_eq!(errors[0]["index"], 1);
    assert_eq!(errors[1]["key"], "no_such_key");
    assert_eq!(errors[1]["code"], "UNKNOWN_KEY");
    assert_eq!(writes(&mock), 0);
    // A single failing key keeps its own code.
    let e = t
        .studio_set_settings(args(vec![change("wall_loops", json!("many"))], rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidValue);
}

#[tokio::test]
async fn invalid_batch_size_targets_and_duplicates_are_rejected() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(vec![], rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
    let many = (0..101).map(|_| change("wall_loops", json!(3))).collect();
    let e = t
        .studio_set_settings(args(many, rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
    let mut plate = change("wall_loops", json!(3));
    plate.scope = Some(Scope::Plate);
    let e = t
        .studio_set_settings(args(vec![plate], rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
    let e = t
        .studio_set_settings(args(
            vec![
                change("wall_loops", json!(3)),
                change("wall_loops", json!(4)),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
    assert_eq!(writes(&mock), 0);
}

#[tokio::test]
async fn modal_open_returns_busy_without_write() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    mock.set_modal(true);
    let e = t
        .studio_set_settings(args(demo(), rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
    assert_eq!(e.details["reason"], "modal_open");
    assert_eq!(writes(&mock), 0);
}

#[tokio::test]
async fn version_mismatch_blocks_write() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    // A restart is what makes the probe forget the identity it cached.
    mock.set_session("restarted");
    mock.set_identity("0.0.0.0", "other-patch");
    let e = t
        .studio_set_settings(args(demo(), rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::VersionMismatch);
    assert_eq!(writes(&mock), 0);
}

#[tokio::test]
async fn dry_run_returns_diff_and_does_not_write() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let v = t
        .studio_set_settings(args(demo(), rev, Some(true)))
        .await
        .unwrap();
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["revision"], rev);
    assert_eq!(v["applied"], json!([]));
    let d = &v["diff"][0];
    assert_eq!(d["key"], "sparse_infill_density");
    assert_eq!(d["to"], "35%");
    assert_ne!(d["from"], d["to"]);
    assert_eq!(d["changed"], true);
    assert_eq!(writes(&mock), 0);
    assert_eq!(revision(&t).await, rev);
    // The revision is still checked.
    mock.user_edit("wall_loops", "9");
    let e = t
        .studio_set_settings(args(demo(), rev, Some(true)))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    // And so is writability.
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![change("ironing_type", json!("top"))],
            rev,
            Some(true),
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::NotWritableByDriver);
    assert_eq!(writes(&mock), 0);
}

#[tokio::test]
async fn mixed_scopes_write_once_per_target() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("top_shell_layers", "3");
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let v = t
        .studio_set_settings(args(
            vec![
                change("layer_height", json!(0.16)),
                object_change("sparse_infill_density", json!(50), "obj-1"),
                object_change("wall_loops", json!(6), "obj-1"),
            ],
            rev,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(count(&mock, "settings_update"), 1);
    assert_eq!(count(&mock, "settings_override"), 1);
    assert_eq!(v["applied"][1]["applied"], "50%");
    assert_eq!(v["applied"][1]["target"], json!({"object_id": "obj-1"}));
    assert_eq!(v["applied"][2]["applied"], "6");
}

#[tokio::test]
async fn unknown_object_or_plate_writes_nothing() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![
                change("layer_height", json!(0.16)),
                object_change("wall_loops", json!(3), "nope"),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::ObjectNotFound);
    let e = t
        .studio_set_settings(args(
            vec![
                change("layer_height", json!(0.16)),
                plate_change("wall_loops", json!(3), 7),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PlateNotFound);
    assert_eq!(e.http_status, 404);
    assert_eq!(writes(&mock), 0);
}

#[tokio::test]
async fn plate_only_batch_writes_once_with_a_fresh_plate_revision() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("top_shell_layers", "3");
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let v = t
        .studio_set_settings(args(
            vec![
                plate_change("sparse_infill_density", json!(50), 0),
                plate_change("wall_loops", json!(5), 0),
            ],
            rev,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(count(&mock, "settings_update"), 0);
    assert_eq!(count(&mock, "settings_override"), 1);
    assert_eq!(v["applied"][0]["applied"], "50%");
    assert_eq!(v["applied"][0]["scope"], "plate");
    assert_eq!(v["applied"][0]["target"], json!({"plate_id": 0}));
    assert_eq!(v["applied"][1]["applied"], "5");
    // Read back from the plate itself.
    let g = t
        .studio_get_settings(bamburino_core::tools::GetSettingsArgs {
            scope: Scope::Plate,
            target: Some(TargetRef {
                plate_id: Some(0),
                object_id: None,
            }),
        })
        .await
        .unwrap();
    let s = g["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "sparse_infill_density")
        .unwrap();
    assert_eq!(s["value"], "50%");
    assert_eq!(s["origin"], "plate");
}

#[tokio::test]
async fn project_then_plate_rereads_the_plate_revision() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("top_shell_layers", "3");
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    // The project write takes a snapshot and so invalidates every plate
    // revision issued before it; the plate write must use a new one.
    let v = t
        .studio_set_settings(args(
            vec![
                change("layer_height", json!(0.16)),
                plate_change("sparse_infill_density", json!(50), 0),
            ],
            rev,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(count(&mock, "settings_update"), 1);
    assert_eq!(count(&mock, "settings_override"), 1);
    assert_eq!(v["applied"][1]["applied"], "50%");
    // And the other way round.
    let rev = v["revision"].as_u64().unwrap();
    t.studio_set_settings(args(
        vec![
            plate_change("wall_loops", json!(5), 0),
            change("layer_height", json!(0.12)),
        ],
        rev,
        None,
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn dry_run_on_plate_does_not_write() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let v = t
        .studio_set_settings(args(
            vec![plate_change("sparse_infill_density", json!(50), 0)],
            rev,
            Some(true),
        ))
        .await
        .unwrap();
    assert_eq!(v["diff"][0]["to"], "50%");
    assert_eq!(v["diff"][0]["target"], json!({"plate_id": 0}));
    assert_eq!(writes(&mock), 0);
    let e = t
        .studio_set_settings(args(
            vec![plate_change("wall_loops", json!(5), 9)],
            rev,
            Some(true),
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PlateNotFound);
}

/// Wraps the mock and injects faults around the write calls.
struct Flaky {
    inner: Arc<MockDriver>,
    fail_update: Option<DriverError>,
    fail_override: Option<DriverError>,
    /// Runs inside `settings_update`, before the write reaches the mock.
    before_update: Option<Box<dyn Fn() + Send + Sync>>,
    /// Runs after a successful `settings_update`.
    after_update: Option<Box<dyn Fn() + Send + Sync>>,
    /// Runs once, right after the next `app_state` call returned.
    after_app_state: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Flaky {
    fn new(inner: &Arc<MockDriver>) -> Self {
        Self {
            inner: inner.clone(),
            fail_update: None,
            fail_override: None,
            before_update: None,
            after_update: None,
            after_app_state: std::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl DriverClient for Flaky {
    async fn app_state(&self) -> Result<AppState, DriverError> {
        let state = self.inner.app_state().await;
        let hook = self.after_app_state.lock().unwrap().take();
        if let Some(f) = hook {
            f();
        }
        state
    }
    async fn capabilities(&self) -> Result<DriverCapabilities, DriverError> {
        self.inner.capabilities().await
    }
    async fn plate_list(&self) -> Result<PlateList, DriverError> {
        self.inner.plate_list().await
    }
    async fn plate_select(&self, i: u32, r: &str) -> Result<PlateList, DriverError> {
        self.inner.plate_select(i, r).await
    }
    async fn model_list(&self) -> Result<Vec<ModelObject>, DriverError> {
        self.inner.model_list().await
    }
    async fn active_presets(&self) -> Result<ActivePresets, DriverError> {
        self.inner.active_presets().await
    }
    async fn writable_keys(&self) -> Result<BTreeSet<String>, DriverError> {
        self.inner.writable_keys().await
    }
    async fn settings_effective(
        &self,
        target: &SettingsTarget,
        keys: Option<&[String]>,
    ) -> Result<Vec<EffectiveSetting>, DriverError> {
        self.inner.settings_effective(target, keys).await
    }
    async fn settings_update(&self, values: &BTreeMap<String, String>) -> Result<(), DriverError> {
        if let Some(e) = &self.fail_update {
            return Err(e.clone());
        }
        if let Some(f) = &self.before_update {
            f();
        }
        self.inner.settings_update(values).await?;
        if let Some(f) = &self.after_update {
            f();
        }
        Ok(())
    }
    async fn settings_override(
        &self,
        target: &SettingsTarget,
        values: &BTreeMap<String, String>,
    ) -> Result<PlateList, DriverError> {
        if let Some(e) = &self.fail_override {
            return Err(e.clone());
        }
        self.inner.settings_override(target, values).await
    }
    async fn object_transform(
        &self,
        o: &str,
        i: &str,
        t: &AbsoluteTransform,
    ) -> Result<ModelObject, DriverError> {
        self.inner.object_transform(o, i, t).await
    }
    async fn slice_start(&self, p: u32, r: &str) -> Result<DriverJob, DriverError> {
        self.inner.slice_start(p, r).await
    }
    async fn job_get(&self, id: &str) -> Result<DriverJob, DriverError> {
        self.inner.job_get(id).await
    }
    async fn job_cancel(&self, id: &str) -> Result<DriverJob, DriverError> {
        self.inner.job_cancel(id).await
    }
    async fn slice_state(&self) -> Result<SliceState, DriverError> {
        self.inner.slice_state().await
    }
    async fn preview_summary(&self, p: u32, r: &str, s: &str) -> Result<SliceSummary, DriverError> {
        self.inner.preview_summary(p, r, s).await
    }
    async fn export_gcode(
        &self,
        p: u32,
        r: &str,
        s: &str,
        path: &std::path::Path,
    ) -> Result<DriverJob, DriverError> {
        self.inner.export_gcode(p, r, s, path).await
    }
}

#[tokio::test]
async fn driver_timeout_maps_to_busy_unknown_outcome() {
    let mock = Arc::new(MockDriver::new());
    let mut flaky = Flaky::new(&mock);
    flaky.fail_update = Some(DriverError::Timeout { after_ms: 35_000 });
    let t = tools_over(Arc::new(flaky));
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(demo(), rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
    assert_eq!(e.details["outcome"], "unknown");
}

#[tokio::test]
async fn second_write_failure_is_partially_applied() {
    let mock = Arc::new(MockDriver::new());
    let mut flaky = Flaky::new(&mock);
    flaky.fail_override = Some(DriverError::Native {
        code: "X".to_string(),
        message: "boom".to_string(),
    });
    let t = tools_over(Arc::new(flaky));
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![
                change("layer_height", json!(0.16)),
                object_change("wall_loops", json!(6), "obj-1"),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PartiallyApplied);
    assert_eq!(e.http_status, 207);
    assert_eq!(e.details["applied"][0]["key"], "layer_height");
    assert_eq!(e.details["applied"][0]["applied"], "0.16");
    assert_eq!(e.details["failed"][0]["keys"], json!(["wall_loops"]));
    let new_rev = e.details["revision"].as_u64().unwrap();
    assert!(new_rev > rev);
    // The revision in the payload is live: nothing is mistaken for a user edit.
    assert_eq!(revision(&t).await, new_rev);
    assert_eq!(count(&mock, "settings_update"), 1);
}

#[tokio::test]
async fn edit_between_the_state_read_and_the_refresh_is_a_conflict() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("wall_loops", "2"); // the snapshot id is now Some
    let flaky = Arc::new(Flaky::new(&mock));
    let t = tools_over(flaky.clone());
    let rev = revision(&t).await;
    // The user edits right after the pipeline read `app_state` (and with it
    // the pre-mutation `undo_snapshot`), before its revision refresh.
    let m2 = mock.clone();
    *flaky.after_app_state.lock().unwrap() =
        Some(Box::new(move || m2.user_edit("top_shell_layers", "5")));
    let e = t
        .studio_set_settings(args(vec![change("layer_height", json!(0.16))], rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(e.http_status, 409);
    assert_eq!(e.details["reason"], "revision_mismatch");
    assert!(e.details["current"].as_u64().unwrap() > rev);
    assert_eq!(writes(&mock), 0);
}

#[tokio::test]
async fn undo_during_the_write_reports_applied_and_do_not_retry() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("wall_loops", "2");
    let mut flaky = Flaky::new(&mock);
    // The user presses Ctrl+Z right after the write: the active snapshot ends
    // up before where the write alone would have put it.
    let m2 = mock.clone();
    flaky.after_update = Some(Box::new(move || m2.user_undo()));
    let t = tools_over(Arc::new(flaky));
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(vec![change("layer_height", json!(0.16))], rev, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(e.details["reason"], "undo_snapshot_mismatch");
    assert_eq!(e.details["outcome"], "applied");
    assert_eq!(e.details["applied"][0]["key"], "layer_height");
    assert!(e.details["applied"][0]["applied"].is_string());
    assert!(e.message.contains("Do not retry blindly"));
    assert!(e.details["current"].as_u64().unwrap() >= rev);
}

#[tokio::test]
async fn write_after_a_user_undo_is_not_a_false_conflict() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("wall_loops", "2");
    mock.user_edit("top_shell_layers", "4");
    mock.user_undo(); // active snapshot goes back; the next one jumps over it
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let v = t
        .studio_set_settings(args(demo(), rev, None))
        .await
        .unwrap();
    assert_eq!(v["applied"][2]["applied"], "4");
    // And again with two writes.
    mock.user_undo();
    let rev = revision(&t).await;
    t.studio_set_settings(args(
        vec![
            change("layer_height", json!(0.16)),
            object_change("wall_loops", json!(6), "obj-1"),
        ],
        rev,
        None,
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn partial_failure_also_runs_the_snapshot_check() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("wall_loops", "2");
    let mut flaky = Flaky::new(&mock);
    flaky.fail_override = Some(DriverError::Native {
        code: "X".to_string(),
        message: "boom".to_string(),
    });
    let m2 = mock.clone();
    flaky.after_update = Some(Box::new(move || m2.user_undo()));
    let t = tools_over(Arc::new(flaky));
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![
                change("layer_height", json!(0.16)),
                object_change("wall_loops", json!(6), "obj-1"),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PartiallyApplied);
    assert!(e.details["concurrent_edit"]["undo_snapshot"]["after"].is_string());
}

#[tokio::test]
async fn stale_reference_on_a_plate_override_is_409_or_207() {
    let stale = || DriverError::StaleReference {
        message: "plate revision is outdated".to_string(),
    };
    // First (only) group: nothing written, standard 409 shape.
    let mock = Arc::new(MockDriver::new());
    let mut flaky = Flaky::new(&mock);
    flaky.fail_override = Some(stale());
    let t = tools_over(Arc::new(flaky));
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![plate_change("sparse_infill_density", json!(50), 0)],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(e.http_status, 409);
    let current = e.details["current"].as_u64().unwrap();
    assert_eq!(e.details["expected"], rev);
    assert_eq!(e.details["state"]["revision"].as_u64().unwrap(), current);

    // Second group: the project part is applied, so 207.
    let mock = Arc::new(MockDriver::new());
    let mut flaky = Flaky::new(&mock);
    flaky.fail_override = Some(stale());
    let t = tools_over(Arc::new(flaky));
    let rev = revision(&t).await;
    let e = t
        .studio_set_settings(args(
            vec![
                change("layer_height", json!(0.16)),
                plate_change("sparse_infill_density", json!(50), 0),
            ],
            rev,
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PartiallyApplied);
    assert_eq!(e.http_status, 207);
    assert_eq!(e.details["applied"][0]["key"], "layer_height");
    assert_eq!(e.details["failed"][0]["error"]["code"], "REVISION_CONFLICT");
    assert_eq!(e.details["failed"][0]["target"], json!({"plate_id": 0}));
}

#[tokio::test]
async fn undisturbed_write_passes_the_snapshot_check() {
    let mock = Arc::new(MockDriver::new());
    mock.user_edit("wall_loops", "2");
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    // Two writes (project + object) advance the id by two: still accepted.
    t.studio_set_settings(args(
        vec![
            change("layer_height", json!(0.16)),
            object_change("wall_loops", json!(6), "obj-1"),
        ],
        rev,
        None,
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn snapshot_check_is_skipped_without_driver_support() {
    let mock = Arc::new(MockDriver::new());
    mock.set_undo_snapshot_supported(false);
    let mut flaky = Flaky::new(&mock);
    let m2 = mock.clone();
    flaky.before_update = Some(Box::new(move || m2.user_edit("top_shell_layers", "5")));
    let t = tools_over(Arc::new(flaky));
    let rev = revision(&t).await;
    // Same interleaving, but there is no `undo_snapshot` to compare.
    t.studio_set_settings(args(vec![change("layer_height", json!(0.16))], rev, None))
        .await
        .unwrap();
}

// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::collections::BTreeMap;

use bamburino_core::driver::mock::{MockAms, MockArrange, MockDriver, MockFilament};
use bamburino_core::driver::{
    AmsTray, AmsUnit, DeviceSummary, DriverClient, DriverError, JobStatus, PlateContents,
    SettingSource, SettingsTarget,
};

fn filament(preset: &str, material: &str, colour: Option<&str>) -> MockFilament {
    MockFilament {
        preset: preset.to_string(),
        material: material.to_string(),
        colour: colour.map(str::to_string),
    }
}

fn two_filaments(m: &MockDriver) {
    m.set_filaments(vec![
        filament("Bambu PLA Basic", "PLA", Some("#00AE42")),
        filament("Bambu PETG Basic", "PETG", Some("#FF0000")),
    ]);
}

fn invalid(err: DriverError, needle: &str) {
    match err {
        DriverError::InvalidParams { message } => {
            assert!(message.contains(needle), "{message:?} lacks {needle:?}")
        }
        other => panic!("expected InvalidParams({needle}), got {other:?}"),
    }
}

async fn undo_snapshot(m: &MockDriver) -> Option<String> {
    m.app_state().await.unwrap().undo_snapshot
}

async fn slice_selected(m: &MockDriver) -> String {
    let rev = m.plate_list().await.unwrap().plate_revision;
    let job = m.slice_start(0, &rev).await.unwrap();
    m.job_get(&job.id).await.unwrap();
    m.job_get(&job.id)
        .await
        .unwrap()
        .slice_result_id
        .expect("result id")
}

#[tokio::test]
async fn plate_create_appends_a_plate_and_stales_old_revisions() {
    let m = MockDriver::new();
    let rev0 = m.plate_list().await.unwrap().plate_revision;
    let list = m.plate_create().await.unwrap();
    assert_eq!(list.plates.len(), 2);
    assert_eq!(list.plates[1].index, 1);
    assert!(matches!(
        m.plate_rename(0, &rev0, "x").await,
        Err(DriverError::StaleReference { .. })
    ));
    let renamed = m.plate_rename(0, &list.plate_revision, "x").await.unwrap();
    assert_eq!(renamed.plates[0].name, "x");
    assert_eq!(renamed.plates[1].name, "Plate 2");
}

#[tokio::test]
async fn plate_delete_refusals_use_the_driver_messages() {
    let m = MockDriver::new();
    let rev = m.plate_list().await.unwrap().plate_revision;
    let err = m
        .plate_delete(0, &rev, PlateContents::Delete, None)
        .await
        .unwrap_err();
    invalid(err, "Cannot delete the last plate");

    m.set_plate_count(3);
    let rev = m.plate_list().await.unwrap().plate_revision;
    let err = m
        .plate_delete(0, &rev, PlateContents::RequireEmpty, None)
        .await
        .unwrap_err();
    invalid(err, "choose contents=move or contents=delete");

    m.lock_plate(2, true);
    let rev = m.plate_list().await.unwrap().plate_revision;
    let err = m
        .plate_delete(2, &rev, PlateContents::RequireEmpty, None)
        .await
        .unwrap_err();
    invalid(err, "Unlock the plate");

    // Move to a non-empty target, then to a locked one.
    let err = m
        .plate_delete(0, &rev, PlateContents::Move, Some(0))
        .await
        .unwrap_err();
    invalid(err, "another plate");
    m.add_object("obj-2", "other", 1);
    let rev = m.plate_list().await.unwrap().plate_revision;
    let err = m
        .plate_delete(0, &rev, PlateContents::Move, Some(1))
        .await
        .unwrap_err();
    invalid(err, "Move destination must be unlocked and empty");
    let err = m
        .plate_delete(0, &rev, PlateContents::Move, Some(2))
        .await
        .unwrap_err();
    invalid(err, "Move destination must be unlocked and empty");
}

#[tokio::test]
async fn plate_delete_move_renumbers_and_reports_destination() {
    let m = MockDriver::new();
    m.set_plate_count(3);
    // obj-1 sits on plate 0 by default; put it on plate 1.
    let rev = m.plate_list().await.unwrap().plate_revision;
    m.plate_move_instance("obj-1", "inst-1", 1, &rev)
        .await
        .unwrap();
    let rev = m.plate_list().await.unwrap().plate_revision;
    let done = m
        .plate_delete(1, &rev, PlateContents::Move, Some(2))
        .await
        .unwrap();
    assert_eq!(done.destination_plate_index, Some(1));
    assert_eq!(done.moved_instances, 1);
    assert_eq!(done.deleted_objects, 0);
    assert_eq!(done.plates.plates.len(), 2);
    assert_eq!(m.instances_on_plate(1), [("obj-1".into(), "inst-1".into())]);
    assert!(m.instances_on_plate(0).is_empty());
}

#[tokio::test]
async fn plate_delete_delete_removes_objects_left_without_instances() {
    let m = MockDriver::new();
    m.set_plate_count(2);
    let rev = m.plate_list().await.unwrap().plate_revision;
    let done = m
        .plate_delete(0, &rev, PlateContents::Delete, None)
        .await
        .unwrap();
    assert_eq!(done.deleted_objects, 1);
    assert_eq!(done.moved_instances, 0);
    assert_eq!(done.destination_plate_index, None);
    assert!(m.model_list().await.unwrap().is_empty());
    assert_eq!(m.app_state().await.unwrap().object_count, 0);
    assert_eq!(m.app_state().await.unwrap().plate_count, 1);
}

#[tokio::test]
async fn plate_move_instance_snapshots_only_when_the_plate_changes() {
    let m = MockDriver::new();
    m.set_plate_count(2);
    let before = undo_snapshot(&m).await;
    let rev = m.plate_list().await.unwrap().plate_revision;
    m.plate_move_instance("obj-1", "inst-1", 1, &rev)
        .await
        .unwrap();
    let moved = undo_snapshot(&m).await;
    assert_ne!(before, moved);
    assert_eq!(m.instances_on_plate(1).len(), 1);

    let rev = m.plate_list().await.unwrap().plate_revision;
    m.plate_move_instance("obj-1", "inst-1", 1, &rev)
        .await
        .unwrap();
    assert_eq!(undo_snapshot(&m).await, moved);
}

#[tokio::test]
async fn move_to_locked_plate_is_refused() {
    let m = MockDriver::new();
    m.set_plate_count(2);
    m.lock_plate(1, true);
    let rev = m.plate_list().await.unwrap().plate_revision;
    let err = m
        .plate_move_instance("obj-1", "inst-1", 1, &rev)
        .await
        .unwrap_err();
    invalid(err, "Unlock both plates");
    assert!(matches!(
        m.plate_move_instance("nope", "inst-1", 1, &rev).await,
        Err(DriverError::StaleReference { .. })
    ));
}

#[tokio::test]
async fn filament_assign_writes_extruder_and_reads_back_through_settings_effective() {
    let m = MockDriver::new();
    two_filaments(&m);
    let before = m.model_list().await.unwrap();
    let done = m.filament_assign("obj-1", 1).await.unwrap();
    assert_eq!(done.filament_index, 1);
    assert_eq!(done.preset, "Bambu PETG Basic");
    assert_eq!(m.extruder_of("obj-1"), 2);
    let target = SettingsTarget::Object {
        object_id: "obj-1".into(),
    };
    let got = m
        .settings_effective(&target, Some(&["extruder".to_string()]))
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        (got[0].value.as_str(), got[0].source),
        ("2", SettingSource::Object)
    );
    // The revision fingerprint reads all keys: they must include extruder.
    let all = m.settings_effective(&target, None).await.unwrap();
    assert!(all.iter().any(|s| s.key == "extruder" && s.value == "2"));
    assert_eq!(before, m.model_list().await.unwrap());

    let err = m.filament_assign("obj-1", 2).await.unwrap_err();
    assert!(matches!(err, DriverError::InvalidParams { .. }), "{err:?}");
    assert!(matches!(
        m.filament_assign("ghost", 0).await,
        Err(DriverError::StaleReference { .. })
    ));
    assert_eq!(m.assigned_filament_indices(), [1]);
}

#[tokio::test]
async fn project_colours_are_readable_as_semicolon_lists() {
    let m = MockDriver::new();
    two_filaments(&m);
    let keys = ["filament_colour".to_string(), "filament_type".to_string()];
    let got = m
        .settings_effective(&SettingsTarget::Project, Some(&keys))
        .await
        .unwrap();
    assert_eq!(got[0].value, "#00AE42;#FF0000");
    assert_eq!(got[1].value, "PLA;PETG");
    assert_eq!(got[0].source, SettingSource::Project);

    m.set_filaments(vec![
        filament("A", "PLA", None),
        filament("B", "PLA", Some("#112233")),
    ]);
    let got = m
        .settings_effective(&SettingsTarget::Project, Some(&keys))
        .await
        .unwrap();
    assert_eq!(got[0].value, ";#112233");
    let presets = m.active_presets().await.unwrap();
    assert_eq!(presets.filaments, ["A", "B"]);
}

#[tokio::test]
async fn arrange_job_needs_two_polls_and_shifts_instances() {
    let m = MockDriver::new();
    m.set_arrange(MockArrange::Shift([10.0, 5.0, 0.0]));
    let result = slice_selected(&m).await;
    assert!(m.slice_state().await.unwrap().valid);
    let before = undo_snapshot(&m).await;
    let pos0 = m.model_list().await.unwrap()[0].instances[0].position_mm;

    let job = m.arrange_start().await.unwrap();
    assert_eq!(job.status, JobStatus::Running);
    assert_ne!(undo_snapshot(&m).await, before);
    assert!(!m.slice_state().await.unwrap().valid);
    // The mutation gate: a running arrange blocks further mutations.
    assert!(matches!(
        m.plate_create().await,
        Err(DriverError::Busy { .. })
    ));
    let first = m.job_get(&job.id).await.unwrap();
    assert_eq!(first.status, JobStatus::Running);
    let second = m.job_get(&job.id).await.unwrap();
    assert_eq!(second.status, JobStatus::Succeeded);
    assert_eq!(second.slice_result_id, None);
    let pos1 = m.model_list().await.unwrap()[0].instances[0].position_mm;
    assert_eq!(pos1, [pos0[0] + 10.0, pos0[1] + 5.0, pos0[2]]);
    assert!(!m.slice_state().await.unwrap().valid);
    let _ = result;
}

#[tokio::test]
async fn arrange_shift_and_swap_plates_moves_instances_between_plates() {
    let m = MockDriver::new();
    m.set_plate_count(2);
    m.set_arrange(MockArrange::ShiftAndSwapPlates);
    let job = m.arrange_start().await.unwrap();
    m.job_get(&job.id).await.unwrap();
    assert_eq!(
        m.job_get(&job.id).await.unwrap().status,
        JobStatus::Succeeded
    );
    assert!(m.instances_on_plate(0).is_empty());
    assert_eq!(m.instances_on_plate(1).len(), 1);
}

#[tokio::test]
async fn arrange_fail_reports_a_string_error_job() {
    let m = MockDriver::new();
    m.set_arrange(MockArrange::Fail("boom".into()));
    let job = m.arrange_start().await.unwrap();
    m.job_get(&job.id).await.unwrap();
    let done = m.job_get(&job.id).await.unwrap();
    assert_eq!(done.status, JobStatus::Failed);
    assert_eq!(done.error.unwrap().message, "boom");
}

#[tokio::test]
async fn ams_and_devices_modes() {
    let m = MockDriver::new();
    assert!(m.device_list().await.unwrap().is_empty());
    assert!(m.ams_inventory("dev-1").await.unwrap().units.is_empty());

    let unit = AmsUnit {
        ams_id: "0".into(),
        trays: vec![AmsTray {
            slot_id: "0".into(),
            exists: true,
            filament_type: "PLA".into(),
            color: "FF0000FF".into(),
            remaining_percent: Some(80),
        }],
    };
    m.set_ams(MockAms::Units(vec![unit.clone()]));
    assert_eq!(m.ams_inventory("dev-1").await.unwrap().units, [unit]);

    m.set_ams(MockAms::Absent);
    assert!(m.ams_inventory("dev-1").await.unwrap().units.is_empty());

    m.set_ams(MockAms::Offline);
    assert!(matches!(
        m.ams_inventory("dev-1").await,
        Err(DriverError::StaleReference { .. })
    ));

    let dev = DeviceSummary {
        device_id: "dev-1".into(),
        name: "X1C".into(),
        selected: true,
        online: true,
        connected: true,
    };
    m.set_devices(vec![dev.clone()]);
    assert_eq!(m.device_list().await.unwrap(), [dev]);
}

#[tokio::test]
async fn project_save_and_export_write_files_and_enforce_flags() {
    let m = MockDriver::new();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("p.3mf");

    // project_save refusals
    invalid(
        m.project_save(std::path::Path::new("rel.3mf"), false)
            .await
            .unwrap_err(),
        "absolute .3mf",
    );
    invalid(
        m.project_save(&dir.path().join("p.stl"), false)
            .await
            .unwrap_err(),
        "absolute .3mf",
    );
    invalid(
        m.project_save(&dir.path().join("p.3MF"), false)
            .await
            .unwrap_err(),
        "absolute .3mf",
    );
    invalid(
        m.project_save(&dir.path().join("nodir").join("p.3mf"), false)
            .await
            .unwrap_err(),
        "directory does not exist",
    );

    m.user_edit("layer_height", "0.16");
    assert!(m.app_state().await.unwrap().dirty);
    let snap = undo_snapshot(&m).await;
    m.project_save(&target, false).await.unwrap();
    let bytes = std::fs::read(&target).unwrap();
    assert!(bytes.starts_with(b"PK"));
    assert_eq!(undo_snapshot(&m).await, snap, "save takes no snapshot");
    let st = m.app_state().await.unwrap();
    assert!(!st.dirty);
    assert_eq!(st.project_path, target.to_string_lossy());

    invalid(
        m.project_save(&target, false).await.unwrap_err(),
        "overwrite=true",
    );
    m.project_save(&target, true).await.unwrap();

    // export
    let out = dir.path().join("plate.gcode.3mf");
    let rev = m.plate_list().await.unwrap().plate_revision;
    match m
        .export_sliced_3mf(0, &rev, "mock-result-x", &out, false)
        .await
    {
        Err(DriverError::StaleReference { message }) => {
            assert!(message.contains("no longer has the requested completed slice"))
        }
        other => panic!("expected StaleReference, got {other:?}"),
    }
    let result = slice_selected(&m).await;
    let rev = m.plate_list().await.unwrap().plate_revision;
    invalid(
        m.export_sliced_3mf(0, &rev, &result, &dir.path().join("a.3mf"), false)
            .await
            .unwrap_err(),
        ".gcode.3mf",
    );
    let done = m
        .export_sliced_3mf(0, &rev, &result, &out, false)
        .await
        .unwrap();
    assert_eq!(done.slice_result_id, result);
    assert_eq!(done.plate_index, 0);
    assert_eq!(done.bytes, std::fs::metadata(&out).unwrap().len());
    assert!(std::fs::read(&out).unwrap().starts_with(b"PK"));
    invalid(
        m.export_sliced_3mf(0, &rev, &result, &out, false)
            .await
            .unwrap_err(),
        "overwrite=true",
    );
    m.export_sliced_3mf(0, &rev, &result, &out, true)
        .await
        .unwrap();

    let upper = dir.path().join("UP.GCODE.3MF");
    m.export_sliced_3mf(0, &rev, &result, &upper, false)
        .await
        .unwrap();
    assert!(upper.exists());

    // writes disabled: success, no file
    m.set_write_files(false);
    let quiet = dir.path().join("quiet.gcode.3mf");
    m.export_sliced_3mf(0, &rev, &result, &quiet, false)
        .await
        .unwrap();
    assert!(!quiet.exists());
    let quiet_proj = dir.path().join("quiet.3mf");
    m.project_save(&quiet_proj, false).await.unwrap();
    assert!(!quiet_proj.exists());

    // every received flag, including refused calls: 8 project_save + 7 export
    let flags = m.file_overwrite_flags();
    assert_eq!(flags.iter().filter(|f| **f).count(), 2);
    assert_eq!(flags.len(), 15);
}

#[tokio::test]
async fn existing_single_cube_behaviour_is_unchanged() {
    let m = MockDriver::new();
    assert_eq!(m.app_state().await.unwrap().plate_count, 1);
    assert_eq!(m.app_state().await.unwrap().project_path, "");
    let objects = m.model_list().await.unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].id, "obj-1");
    assert_eq!(objects[0].instances[0].id, "inst-1");
    assert_eq!(objects[0].instances[0].plate_index, 0);
    assert_eq!(m.extruder_of("obj-1"), 1);
    assert_eq!(m.active_presets().await.unwrap().filaments, ["Mock PLA"]);
    let keys: BTreeMap<String, String> = BTreeMap::new();
    let _ = keys;
}

#[tokio::test]
async fn added_objects_and_raw_rotation() {
    let m = MockDriver::new();
    m.set_plate_count(2);
    m.add_object("obj-2", "second", 1);
    m.add_instance("obj-2", "inst-extra", 0);
    m.set_instance_rotation("obj-2", "inst-obj-2", [-0.0, 0.0, 90.0]);
    let objects = m.model_list().await.unwrap();
    assert_eq!(objects.len(), 2);
    assert_eq!(objects[1].name, "second");
    assert_eq!(objects[1].instances.len(), 2);
    assert_eq!(objects[1].instances[0].id, "inst-obj-2");
    assert_eq!(objects[1].instances[0].plate_index, 1);
    assert!(objects[1].instances[0].rotation_deg[0].is_sign_negative());
    assert_eq!(
        m.instances_on_plate(0),
        [
            ("obj-1".to_string(), "inst-1".to_string()),
            ("obj-2".to_string(), "inst-extra".to_string())
        ]
    );
}

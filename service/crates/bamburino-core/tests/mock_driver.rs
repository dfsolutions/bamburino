// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::collections::BTreeMap;
use std::time::Duration;

use bamburino_core::build_info;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::driver::{
    DriverClient, DriverError, DriverTool, JobStatus, SettingSource, SettingsTarget,
};

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn allowlist_has_27_tools_and_only_read_only_device_list() {
    assert_eq!(DriverTool::ALL.len(), 27);
    let device: Vec<_> = DriverTool::ALL
        .iter()
        .map(|t| t.name())
        .filter(|n| n.starts_with("device_"))
        .collect();
    assert_eq!(device, ["device_list"]);
    assert!(!DriverTool::DeviceList.is_mutation() && !DriverTool::AmsInventory.is_mutation());
    for banned in [
        "printer_",
        "calibration_",
        "account_",
        "history_",
        "preferences_",
        "ams_mapping",
    ] {
        assert!(
            DriverTool::ALL
                .iter()
                .all(|t| !t.name().starts_with(banned)),
            "{banned}"
        );
    }
    assert!(DriverTool::ALL.iter().all(|t| t.name() != "preset_select"));
}

#[test]
fn exports_are_gcode_and_sliced_3mf_and_project_save_is_the_only_project_tool() {
    let exports: Vec<_> = DriverTool::ALL
        .iter()
        .map(|t| t.name())
        .filter(|n| n.contains("export"))
        .collect();
    assert_eq!(exports, ["export_gcode", "export_sliced_3mf"]);
    let project: Vec<_> = DriverTool::ALL
        .iter()
        .map(|t| t.name())
        .filter(|n| n.starts_with("project_"))
        .collect();
    assert_eq!(project, ["project_save"]);
    for t in [
        DriverTool::ProjectSave,
        DriverTool::ExportSliced3mf,
        DriverTool::ArrangeStart,
        DriverTool::PlateCreate,
        DriverTool::PlateDelete,
        DriverTool::PlateRename,
        DriverTool::PlateMoveInstance,
        DriverTool::FilamentAssign,
    ] {
        assert!(t.is_mutation(), "{}", t.name());
    }
}

#[test]
fn driver_tool_names_are_exact() {
    assert_eq!(DriverTool::SettingsUpdate.name(), "settings_update");
    assert_eq!(DriverTool::PreviewGetSummary.name(), "preview_get_summary");
    assert!(DriverTool::SettingsUpdate.is_mutation());
    assert!(!DriverTool::PlateList.is_mutation());
}

#[tokio::test]
async fn mock_rejects_non_writable_key() {
    let d = MockDriver::new();
    let err = d
        .settings_update(&map(&[("filament_type", "PLA")]))
        .await
        .unwrap_err();
    assert!(matches!(err, DriverError::InvalidParams { .. }), "{err:?}");
    // print_sequence is writable globally but not overridable.
    d.settings_update(&map(&[("print_sequence", "by object")]))
        .await
        .unwrap();
    let revision = d.plate_list().await.unwrap().plate_revision;
    let err = d
        .settings_override(
            &SettingsTarget::Plate {
                index: 0,
                plate_revision: revision,
            },
            &map(&[("print_sequence", "by object")]),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DriverError::InvalidParams { .. }), "{err:?}");
}

#[tokio::test]
async fn mock_plate_revision_changes_every_list() {
    let d = MockDriver::new();
    let a = d.plate_list().await.unwrap().plate_revision;
    let b = d.plate_list().await.unwrap().plate_revision;
    assert_ne!(a, b);
}

#[tokio::test]
async fn mock_slice_start_rejects_unissued_and_outdated_revisions() {
    let d = MockDriver::new();
    let err = d.slice_start(0, "bogus").await.unwrap_err();
    assert!(matches!(err, DriverError::StaleReference { .. }), "{err:?}");

    let old = d.plate_list().await.unwrap().plate_revision;
    d.settings_update(&map(&[("layer_height", "0.16")]))
        .await
        .unwrap();
    let err = d.slice_start(0, &old).await.unwrap_err();
    assert!(matches!(err, DriverError::StaleReference { .. }), "{err:?}");

    let fresh = d.plate_list().await.unwrap().plate_revision;
    d.slice_start(0, &fresh).await.unwrap();
}

#[tokio::test]
async fn mock_slice_job_reaches_succeeded_in_two_polls() {
    let d = MockDriver::new();
    let rev = d.plate_list().await.unwrap().plate_revision;
    let job = d.slice_start(0, &rev).await.unwrap();
    assert_eq!(job.status, JobStatus::Running);
    assert!(d.app_state().await.unwrap().slicing_running);

    let first = d.job_get(&job.id).await.unwrap();
    assert_eq!(first.status, JobStatus::Running);
    assert_eq!(first.progress_percent, Some(50));
    assert_eq!(first.slice_result_id, None);

    let second = d.job_get(&job.id).await.unwrap();
    assert_eq!(second.status, JobStatus::Succeeded);
    assert_eq!(second.progress_percent, Some(100));
    let result_id = second.slice_result_id.expect("slice result id");
    assert!(!d.app_state().await.unwrap().slicing_running);

    let state = d.slice_state().await.unwrap();
    assert!(state.valid);
    assert_eq!(state.slice_result_id.as_deref(), Some(result_id.as_str()));

    let rev = d.plate_list().await.unwrap().plate_revision;
    let summary = d.preview_summary(0, &rev, &result_id).await.unwrap();
    // Like the real driver: no print time / filament data unless a test
    // simulates Task 18 data.
    assert_eq!(summary.print_time_s, None);
    assert!(summary.filaments.is_empty());
    assert_eq!(summary.support_volume_mm3, None);
    assert_eq!(state.estimated_time_s, Some(1800.0));

    d.set_summary_stats(
        Some(60.0),
        vec![bamburino_core::driver::FilamentUsage {
            index: 0,
            grams: Some(1.0),
            meters: Some(2.0),
        }],
        Some(3.0),
    );
    let rev = d.plate_list().await.unwrap().plate_revision;
    let summary = d.preview_summary(0, &rev, &result_id).await.unwrap();
    assert_eq!(summary.print_time_s, Some(60.0));
    assert_eq!(summary.filaments.len(), 1);
}

#[tokio::test]
async fn mock_user_undo_restores_previous_values() {
    let d = MockDriver::new();
    let before = d.app_state().await.unwrap().undo_snapshot;
    d.settings_update(&map(&[("layer_height", "0.12")]))
        .await
        .unwrap();
    let after_update = d.app_state().await.unwrap().undo_snapshot;
    assert_ne!(before, after_update);
    d.user_edit("layer_height", "0.3");
    let after_edit = d.app_state().await.unwrap().undo_snapshot;
    d.user_undo();

    let eff = d
        .settings_effective(
            &SettingsTarget::Project,
            Some(&["layer_height".to_string()]),
        )
        .await
        .unwrap();
    assert_eq!(eff.len(), 1);
    assert_eq!(eff[0].value, "0.12");
    assert_eq!(eff[0].source, SettingSource::Project);
    // Like the real stack: undo moves the active time back to the snapshot
    // it restored, which is where the update left it.
    let after_undo = d.app_state().await.unwrap().undo_snapshot;
    assert_ne!(after_undo, after_edit);
    assert_eq!(after_undo, after_update);
}

#[tokio::test]
async fn mock_modal_blocks_mutations_but_not_reads() {
    let d = MockDriver::new();
    d.set_modal(true);
    assert!(d.app_state().await.unwrap().modal_open);
    let err = d
        .settings_update(&map(&[("layer_height", "0.2")]))
        .await
        .unwrap_err();
    assert!(matches!(err, DriverError::Busy { .. }), "{err:?}");
    d.set_modal(false);
    d.settings_update(&map(&[("layer_height", "0.2")]))
        .await
        .unwrap();
}

#[tokio::test]
async fn mock_fail_next_unreachable_and_calls_are_recorded() {
    let d = MockDriver::new();
    d.fail_next(DriverError::Timeout { after_ms: 30_000 });
    assert_eq!(
        d.app_state().await.unwrap_err(),
        DriverError::Timeout { after_ms: 30_000 }
    );
    d.app_state().await.unwrap();

    d.set_unreachable(true);
    assert_eq!(d.plate_list().await.unwrap_err(), DriverError::Unreachable);
    d.set_unreachable(false);

    d.delay_next(Duration::from_millis(20));
    let t = std::time::Instant::now();
    d.model_list().await.unwrap();
    assert!(t.elapsed() >= Duration::from_millis(20));

    assert_eq!(
        d.calls(),
        vec!["app_get_state", "app_get_state", "plate_list", "model_list"]
    );
}

#[tokio::test]
async fn mock_capabilities_identity_is_overridable() {
    let d = MockDriver::new();
    let caps = d.capabilities().await.unwrap();
    assert_eq!(caps.app_version, build_info::STUDIO_VERSION);
    assert_eq!(caps.patch_id.as_deref(), build_info::EXPECTED_PATCH_ID);
    d.set_identity("01.00.00.00", "other-patch");
    let caps = d.capabilities().await.unwrap();
    assert_eq!(caps.app_version, "01.00.00.00");
    assert_eq!(caps.patch_id.as_deref(), Some("other-patch"));
}

#[tokio::test]
async fn mock_object_transform_updates_instance() {
    let d = MockDriver::new();
    let objs = d.model_list().await.unwrap();
    assert_eq!(objs[0].id, "obj-1");
    assert_eq!(objs[0].dimensions_mm, [20.0, 20.0, 20.0]);
    let t = bamburino_core::driver::AbsoluteTransform {
        position_mm: [10.0, 20.0, 0.0],
        rotation_deg: [0.0, 0.0, 45.0],
        scale: [2.0, 2.0, 2.0],
    };
    let o = d.object_transform("obj-1", "inst-1", &t).await.unwrap();
    assert_eq!(o.instances[0].position_mm, [10.0, 20.0, 0.0]);
    assert_eq!(o.dimensions_mm, [40.0, 40.0, 40.0]);
    let err = d.object_transform("nope", "inst-1", &t).await.unwrap_err();
    assert!(matches!(err, DriverError::StaleReference { .. }), "{err:?}");
}

#[tokio::test]
async fn mock_plate_select_is_a_mutation_and_invalidates_revisions() {
    assert!(DriverTool::PlateSelect.is_mutation());
    let d = MockDriver::new();
    let rev = d.plate_list().await.unwrap().plate_revision;
    d.plate_select(0, &rev).await.unwrap();
    let err = d.plate_select(0, &rev).await.unwrap_err();
    assert!(matches!(err, DriverError::StaleReference { .. }), "{err:?}");
}

#[tokio::test]
async fn mock_plate_select_and_override_return_usable_new_revision() {
    let d = MockDriver::new();
    let rev = d.plate_list().await.unwrap().plate_revision;
    let list = d.plate_select(0, &rev).await.unwrap();
    assert_ne!(list.plate_revision, rev);
    let target = SettingsTarget::Plate {
        index: 0,
        plate_revision: list.plate_revision,
    };
    let list = d
        .settings_override(&target, &map(&[("layer_height", "0.1")]))
        .await
        .unwrap();
    // The returned revision is immediately valid for slice_start.
    d.slice_start(0, &list.plate_revision).await.unwrap();
}

#[test]
fn job_status_is_terminal_for_every_variant() {
    for (status, terminal) in [
        (JobStatus::Running, false),
        (JobStatus::Cancelling, false),
        (JobStatus::Succeeded, true),
        (JobStatus::Failed, true),
        (JobStatus::Cancelled, true),
        (JobStatus::Stale, true),
        (JobStatus::Unknown("x".into()), false),
    ] {
        assert_eq!(status.is_terminal(), terminal, "{status:?}");
    }
}

#[tokio::test]
async fn mock_job_cancel_is_a_read_and_works_with_modal_open() {
    assert!(!DriverTool::JobCancel.is_mutation());
    let d = MockDriver::new();
    let rev = d.plate_list().await.unwrap().plate_revision;
    let job = d.slice_start(0, &rev).await.unwrap();
    d.set_modal(true);
    let cancelled = d.job_cancel(&job.id).await.unwrap();
    assert_eq!(cancelled.status, JobStatus::Cancelled);
}

#[tokio::test]
async fn mock_mutations_are_busy_while_a_slice_job_runs() {
    let d = MockDriver::new();
    let rev = d.plate_list().await.unwrap().plate_revision;
    let job = d.slice_start(0, &rev).await.unwrap();
    let err = d
        .settings_update(&map(&[("layer_height", "0.2")]))
        .await
        .unwrap_err();
    assert!(matches!(err, DriverError::Busy { .. }), "{err:?}");
    d.job_get(&job.id).await.unwrap();
    d.job_get(&job.id).await.unwrap();
    d.settings_update(&map(&[("layer_height", "0.2")]))
        .await
        .unwrap();
}

#[tokio::test]
async fn mock_edit_invalidates_slice_result_and_active_jobs() {
    let d = MockDriver::new();
    let rev = d.plate_list().await.unwrap().plate_revision;
    let job = d.slice_start(0, &rev).await.unwrap();
    d.job_get(&job.id).await.unwrap();
    let done = d.job_get(&job.id).await.unwrap();
    let result_id = done.slice_result_id.unwrap();
    assert!(d.slice_state().await.unwrap().valid);

    d.settings_update(&map(&[("layer_height", "0.12")]))
        .await
        .unwrap();
    assert!(!d.slice_state().await.unwrap().valid);
    let rev = d.plate_list().await.unwrap().plate_revision;
    let err = d.preview_summary(0, &rev, &result_id).await.unwrap_err();
    assert!(matches!(err, DriverError::StaleReference { .. }), "{err:?}");

    // An edit made in the GUI while a job runs turns it Stale with a code.
    let job = d.slice_start(0, &rev).await.unwrap();
    d.user_edit("layer_height", "0.3");
    let stale = d.job_get(&job.id).await.unwrap();
    assert_eq!(stale.status, JobStatus::Stale);
    assert_eq!(stale.error.unwrap().code, "STALE_REFERENCE");
    assert!(!d.app_state().await.unwrap().slicing_running);
}

#[test]
fn driver_client_is_object_safe() {
    fn _assert(_: std::sync::Arc<dyn DriverClient>) {}
    _assert(std::sync::Arc::new(MockDriver::new()));
}

#[tokio::test]
async fn mock_object_transform_requires_known_instance() {
    let d = MockDriver::new();
    let t = bamburino_core::driver::AbsoluteTransform {
        position_mm: [0.0; 3],
        rotation_deg: [0.0; 3],
        scale: [1.0; 3],
    };
    let err = d.object_transform("obj-1", "inst-9", &t).await.unwrap_err();
    assert!(matches!(err, DriverError::StaleReference { .. }), "{err:?}");
}

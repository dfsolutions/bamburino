// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::mock::{MockAms, MockDriver, MockFilament};
use bamburino_core::driver::{AmsTray, AmsUnit, DeviceSummary};
use bamburino_core::errors::ToolErrorCode;
use bamburino_core::tools::{
    AssignFilamentArgs, ListFilamentsArgs, ToolConfig, Tools, normalize_ams_colour,
    parse_project_colours,
};
use serde_json::json;

fn tools_with(mock: &Arc<MockDriver>) -> Tools {
    Tools::new(
        mock.clone(),
        Arc::new(FixedProcessProbe(true)),
        ToolConfig {
            allow_version_mismatch: false,
            poll_every: Duration::from_secs(60),
            idle_after: Duration::from_secs(60),
        },
        std::env::temp_dir().join("bamburino-tests-unused"),
    )
}

fn args(id: Option<&str>) -> ListFilamentsArgs {
    ListFilamentsArgs {
        device_id: id.map(String::from),
    }
}

fn fil(preset: &str, material: &str, colour: Option<&str>) -> MockFilament {
    MockFilament {
        preset: preset.into(),
        material: material.into(),
        colour: colour.map(String::from),
    }
}

fn dev(selected: bool, online: bool) -> DeviceSummary {
    DeviceSummary {
        device_id: "d1".into(),
        name: "P1S".into(),
        selected,
        online,
        connected: online,
    }
}

fn units() -> MockAms {
    let tray = |id: &str, exists: bool| AmsTray {
        slot_id: id.into(),
        exists,
        filament_type: if exists { "PLA" } else { "" }.into(),
        color: if exists { "00AE42FF" } else { "00000000" }.into(),
        remaining_percent: Some(if exists { 80 } else { -1 }),
    };
    MockAms::Units(vec![AmsUnit {
        ams_id: "0".into(),
        trays: vec![tray("0", true), tray("1", false)],
    }])
}

#[test]
fn colour_parsers_handle_native_and_ams_formats() {
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        parse_project_colours("#00AE42;#FF0000"),
        vec![some("#00AE42"), some("#FF0000")]
    );
    assert_eq!(
        parse_project_colours("\"#00AE42\";\"#ff0000\""),
        vec![some("#00AE42"), some("#FF0000")]
    );
    assert_eq!(
        parse_project_colours("#00AE42;;#FF0000"),
        vec![some("#00AE42"), None, some("#FF0000")]
    );
    assert!(parse_project_colours("").is_empty());
    assert_eq!(normalize_ams_colour("00AE42FF"), some("#00AE42"));
    assert_eq!(normalize_ams_colour("00AE42"), some("#00AE42"));
    assert_eq!(normalize_ams_colour("#00ae42"), some("#00AE42"));
    assert_eq!(normalize_ams_colour("00000000"), None);
    assert_eq!(normalize_ams_colour(""), None);
    assert_eq!(normalize_ams_colour("zzzzzz"), None);
}

#[tokio::test]
async fn list_filaments_reports_project_slots_one_based_with_colour_and_material() {
    let mock = Arc::new(MockDriver::new());
    mock.set_filaments(vec![
        fil("Bambu PLA Basic", "PLA", Some("#00AE42")),
        fil("Bambu PETG Basic", "PETG", Some("#FF0000")),
    ]);
    let r = tools_with(&mock)
        .studio_list_filaments(args(None))
        .await
        .unwrap();
    assert_eq!(r["project"][0]["slot"], 1);
    assert_eq!(r["project"][1]["slot"], 2);
    assert_eq!(r["project"][0]["preset"], "Bambu PLA Basic");
    assert_eq!(r["project"][1]["material"], "PETG");
    assert_eq!(r["project"][0]["color_hex"], "#00AE42");
    assert_eq!(r["project"][1]["color_hex"], "#FF0000");
    assert!(r["ams"].is_null());
    assert_eq!(r["ams_unavailable_reason"], "no_printer_selected");
}

#[tokio::test]
async fn list_filaments_short_colour_list_gives_null_not_a_guess() {
    let mock = Arc::new(MockDriver::new());
    mock.set_filaments(vec![
        fil("A", "PLA", Some("#00AE42")),
        fil("B", "PETG", None),
    ]);
    let r = tools_with(&mock)
        .studio_list_filaments(args(None))
        .await
        .unwrap();
    assert_eq!(r["project"][0]["color_hex"], "#00AE42");
    assert!(r["project"][1]["color_hex"].is_null());
}

#[tokio::test]
async fn list_filaments_reads_ams_of_the_selected_printer() {
    let mock = Arc::new(MockDriver::new());
    mock.set_devices(vec![dev(true, true)]);
    mock.set_ams(units());
    let r = tools_with(&mock)
        .studio_list_filaments(args(None))
        .await
        .unwrap();
    assert_eq!(r["ams"].as_array().unwrap().len(), 1);
    assert_eq!(
        r["ams"][0],
        json!({"unit":"0","tray":"0","material":"PLA","color_hex":"#00AE42","remaining":80})
    );
    assert!(r["ams_unavailable_reason"].is_null());
    assert_eq!(r["device_id"], "d1");
}

#[tokio::test]
async fn list_filaments_ams_reasons_never_error() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let r = t.studio_list_filaments(args(None)).await.unwrap();
    assert_eq!(r["ams_unavailable_reason"], "no_printer_selected");
    assert!(r["ams"].is_null());

    mock.set_devices(vec![dev(true, false)]);
    let r = t.studio_list_filaments(args(None)).await.unwrap();
    assert_eq!(r["ams_unavailable_reason"], "printer_offline");
    assert!(r["ams"].is_null());

    mock.set_devices(vec![dev(true, true)]);
    mock.set_ams(MockAms::Absent);
    let r = t.studio_list_filaments(args(None)).await.unwrap();
    assert_eq!(r["ams_unavailable_reason"], "no_ams");
    assert!(r["ams"].is_null());

    mock.set_ams(MockAms::Offline);
    let r = t.studio_list_filaments(args(None)).await.unwrap();
    assert_eq!(r["ams_unavailable_reason"], "ams_read_failed");
    assert!(r["ams"].is_null());
    assert!(r["ams_error"].as_str().is_some_and(|m| !m.is_empty()));
}

#[tokio::test]
async fn list_filaments_device_id_overrides_discovery() {
    let mock = Arc::new(MockDriver::new());
    mock.set_ams(units());
    let r = tools_with(&mock)
        .studio_list_filaments(args(Some("d9")))
        .await
        .unwrap();
    assert_eq!(r["ams"].as_array().unwrap().len(), 1);
    assert_eq!(r["device_id"], "d9");
    assert!(!mock.calls().iter().any(|c| c == "device_list"));
}

#[tokio::test]
async fn list_filaments_is_a_read_and_needs_no_revision() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let r = t.studio_list_filaments(args(None)).await.unwrap();
    let state = t.studio_get_state().await.unwrap();
    assert_eq!(r["revision"], state["revision"]);
    let writes = [
        "settings_update",
        "filament_assign",
        "plate_create",
        "plate_rename",
        "plate_delete",
        "plate_move_instance",
        "arrange_start",
        "project_save",
        "export_sliced_3mf",
        "transform_apply",
    ];
    assert!(!mock.calls().iter().any(|c| writes.contains(&c.as_str())));
}

async fn revision(t: &Tools) -> u64 {
    t.studio_get_state().await.unwrap()["revision"]
        .as_u64()
        .unwrap()
}

fn assign(slot: u32, rev: u64) -> AssignFilamentArgs {
    AssignFilamentArgs {
        object_id: "obj-1".into(),
        filament_slot: slot,
        expected_revision: rev,
    }
}

fn two_presets(mock: &Arc<MockDriver>) {
    mock.set_filaments(vec![
        fil("Bambu PLA Basic", "PLA", Some("#00AE42")),
        fil("Bambu PETG Basic", "PETG", Some("#FF0000")),
    ]);
}

fn no_assign_call(mock: &Arc<MockDriver>) {
    assert!(!mock.calls().iter().any(|c| c == "filament_assign"));
}

#[tokio::test]
async fn assign_sends_zero_based_index_and_reads_back_one_based() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let r = t.studio_assign_filament(assign(2, rev)).await.unwrap();
    assert_eq!(r["object_id"], "obj-1");
    assert_eq!(r["filament_slot"], 2);
    assert_eq!(r["previous_slot"], 1);
    assert_eq!(r["preset"], "Bambu PETG Basic");
    assert_eq!(mock.assigned_filament_indices(), vec![1]);
    assert_eq!(mock.extruder_of("obj-1"), 2);
}

#[tokio::test]
async fn assign_bumps_the_revision_and_the_returned_revision_is_current() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let r = t.studio_assign_filament(assign(2, rev)).await.unwrap();
    assert!(r["revision"].as_u64().unwrap() > rev);
    assert_eq!(
        t.studio_get_state().await.unwrap()["revision"],
        r["revision"]
    );
}

#[tokio::test]
async fn assign_slot_zero_and_out_of_range_are_rejected_before_any_write() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    for slot in [0, 3] {
        let e = t
            .studio_assign_filament(assign(slot, rev))
            .await
            .unwrap_err();
        assert_eq!(e.code, ToolErrorCode::InvalidArgument);
        assert_eq!(e.details["filament_slots"], 2);
    }
    no_assign_call(&mock);
    assert!(mock.assigned_filament_indices().is_empty());
}

#[tokio::test]
async fn assign_unknown_object_is_object_not_found() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let mut a = assign(1, rev);
    a.object_id = "nope".into();
    let e = t.studio_assign_filament(a).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::ObjectNotFound);
    assert_eq!(e.details["object_ids"], json!(["obj-1"]));
    no_assign_call(&mock);
}

#[tokio::test]
async fn assign_with_stale_revision_conflicts_and_writes_nothing() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    mock.user_edit("layer_height", "0.3");
    let e = t.studio_assign_filament(assign(2, rev)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(e.details["reason"], "revision_mismatch");
    no_assign_call(&mock);
}

#[tokio::test]
async fn assign_while_modal_is_busy_without_a_write() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    mock.set_modal(true);
    let e = t.studio_assign_filament(assign(2, rev)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
    no_assign_call(&mock);
}

#[tokio::test]
async fn assign_on_a_locked_plate_is_plate_locked_and_records_nothing() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    mock.lock_plate(0, true);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t.studio_assign_filament(assign(2, rev)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PlateLocked);
    assert!(mock.assigned_filament_indices().is_empty());
    assert_eq!(mock.extruder_of("obj-1"), 1);
}

#[tokio::test]
async fn assign_ignored_by_the_driver_is_applied_unverified_not_success() {
    let mock = Arc::new(MockDriver::new());
    two_presets(&mock);
    mock.set_ignore_filament_assign(true);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t.studio_assign_filament(assign(2, rev)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::DriverError);
    assert_eq!(e.details["outcome"], "applied_unverified");
    assert_eq!(e.details["applied_slot"], 1);
    assert_eq!(mock.assigned_filament_indices(), vec![1]);
}

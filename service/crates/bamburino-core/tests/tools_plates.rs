// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::DriverError;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::driver::tool::DriverTool;
use bamburino_core::errors::{ToolError, ToolErrorCode};
use bamburino_core::tools::{
    MoveToPlateArgs, PlateCreateArgs, PlateDeleteArgs, PlateDeleteContents, PlateRenameArgs,
    ToolConfig, Tools,
};

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

/// Default cube (`obj-1`) plus `obj-2`, both on plate 0.
fn fixture() -> Arc<MockDriver> {
    let m = Arc::new(MockDriver::new());
    m.add_object("obj-2", "slide", 0);
    m
}

async fn revision(t: &Tools) -> u64 {
    t.studio_get_state().await.unwrap()["revision"]
        .as_u64()
        .unwrap()
}

fn count(mock: &MockDriver, name: &str) -> usize {
    mock.calls().iter().filter(|c| *c == name).count()
}

fn create(name: Option<&str>, rev: u64) -> PlateCreateArgs {
    PlateCreateArgs {
        name: name.map(String::from),
        expected_revision: rev,
    }
}

fn rename(plate_index: u32, name: &str, rev: u64) -> PlateRenameArgs {
    PlateRenameArgs {
        plate_index,
        name: name.into(),
        expected_revision: rev,
    }
}

fn delete(
    plate_index: u32,
    contents: Option<PlateDeleteContents>,
    target: Option<u32>,
    rev: u64,
) -> PlateDeleteArgs {
    PlateDeleteArgs {
        plate_index,
        contents,
        target_plate_index: target,
        expected_revision: rev,
    }
}

fn move_args(object: &str, instance: Option<&str>, plate_index: u32, rev: u64) -> MoveToPlateArgs {
    MoveToPlateArgs {
        object_id: object.into(),
        instance_id: instance.map(String::from),
        plate_index,
        expected_revision: rev,
    }
}

fn plate_count(state: &serde_json::Value) -> usize {
    state["plates"].as_array().unwrap().len()
}

fn code(e: &ToolError) -> ToolErrorCode {
    e.code
}

#[tokio::test]
async fn create_without_name_appends_the_last_plate() {
    let mock = fixture();
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let out = t.studio_plate_create(create(None, rev)).await.unwrap();
    assert_eq!(out["plate_index"], 1);
    assert_eq!(out["name_applied"], true);
    assert_eq!(out["name"], "Plate 2");
    assert!(out.get("name_error").is_none());
    assert_ne!(out["revision"].as_u64().unwrap(), rev);
    assert_eq!(count(&mock, "plate_rename"), 0);
    assert_eq!(plate_count(&t.studio_get_state().await.unwrap()), 2);
}

#[tokio::test]
async fn create_with_name_renames_through_a_fresh_plate_revision() {
    let mock = fixture();
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let out = t
        .studio_plate_create(create(Some("Red"), rev))
        .await
        .unwrap();
    assert_eq!(out["plate_index"], 1);
    assert_eq!(out["name_applied"], true);
    assert_eq!(out["name"], "Red");
    // create then rename: each needs its own plate-revision read.
    assert_eq!(count(&mock, "plate_rename"), 1);
    assert_eq!(count(&mock, "plate_create"), 1);
    let state = t.studio_get_state().await.unwrap();
    assert_eq!(state["plates"][1]["name"], "Red");
    assert_ne!(out["revision"].as_u64().unwrap(), rev);
    assert_eq!(out["revision"].as_u64().unwrap(), revision(&t).await);
}

#[tokio::test]
async fn create_rename_failure_reports_name_applied_false_and_keeps_the_plate() {
    let mock = fixture();
    mock.fail_tool(
        DriverTool::PlateRename,
        DriverError::Native {
            code: "NATIVE_ERROR".into(),
            message: "rename exploded".into(),
        },
        1,
    );
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let out = t
        .studio_plate_create(create(Some("Red"), rev))
        .await
        .unwrap();
    assert_eq!(out["name_applied"], false);
    assert!(
        out["name_error"]
            .as_str()
            .unwrap()
            .contains("rename exploded")
    );
    assert_eq!(out["plate_index"], 1);
    assert_eq!(plate_count(&t.studio_get_state().await.unwrap()), 2);
}

#[tokio::test]
async fn rename_unknown_plate_is_plate_not_found() {
    let mock = fixture();
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_plate_rename(rename(5, "x", rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateNotFound);
    assert_eq!(count(&mock, "plate_rename"), 0);
    let e = t
        .studio_plate_rename(rename(0, "  ", rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::InvalidArgument);
    let out = t.studio_plate_rename(rename(0, "Base", rev)).await.unwrap();
    assert_eq!(out["name"], "Base");
    assert_eq!(
        t.studio_get_state().await.unwrap()["plates"][0]["name"],
        "Base"
    );
}

#[tokio::test]
async fn delete_non_empty_without_contents_is_plate_not_empty_and_changes_nothing() {
    let mock = fixture();
    mock.set_plate_count(2);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_plate_delete(delete(0, None, None, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateNotEmpty);
    assert_eq!(e.details["instances"].as_array().unwrap().len(), 2);
    assert_eq!(count(&mock, "plate_delete"), 0);
    assert_eq!(plate_count(&t.studio_get_state().await.unwrap()), 2);
    // An empty plate needs no contents.
    let out = t
        .studio_plate_delete(delete(1, None, None, rev))
        .await
        .unwrap();
    assert_eq!(out["contents"], "none");
    assert_eq!(out["plates"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn delete_last_plate_is_last_plate() {
    let mock = fixture();
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_plate_delete(delete(0, Some(PlateDeleteContents::Delete), None, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::LastPlate);
    assert_eq!(count(&mock, "plate_delete"), 0);
}

#[tokio::test]
async fn delete_locked_plate_is_plate_locked() {
    let mock = fixture();
    mock.set_plate_count(2);
    mock.lock_plate(1, true);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_plate_delete(delete(1, None, None, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateLocked);
    assert_eq!(e.details["plate_index"], 1);
    assert_eq!(count(&mock, "plate_delete"), 0);
}

#[tokio::test]
async fn delete_move_requires_an_explicit_empty_unlocked_target() {
    let mock = fixture();
    mock.add_object("obj-3", "other", 1);
    mock.set_plate_count(3);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let mv = Some(PlateDeleteContents::Move);

    let e = t
        .studio_plate_delete(delete(0, mv, None, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::InvalidArgument);
    let e = t
        .studio_plate_delete(delete(0, mv, Some(0), rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::InvalidArgument);
    let e = t
        .studio_plate_delete(delete(0, None, Some(2), rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::InvalidArgument);
    let e = t
        .studio_plate_delete(delete(0, mv, Some(1), rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateNotEmpty);
    assert_eq!(e.details["reason"], "target_not_empty");

    mock.lock_plate(2, true);
    let rev = revision(&t).await;
    let e = t
        .studio_plate_delete(delete(0, mv, Some(2), rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateLocked);
    assert_eq!(e.details["plate_index"], 2);
    let e = t
        .studio_plate_delete(delete(0, mv, Some(9), rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateNotFound);
    assert_eq!(count(&mock, "plate_delete"), 0);

    mock.lock_plate(2, false);
    let rev = revision(&t).await;
    let out = t
        .studio_plate_delete(delete(0, mv, Some(2), rev))
        .await
        .unwrap();
    assert_eq!(out["contents"], "move");
    assert_eq!(out["moved_instances"], 2);
}

#[tokio::test]
async fn delete_contents_delete_removes_objects_wholly_on_the_plate() {
    let mock = fixture();
    mock.add_object("obj-3", "x", 1);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let out = t
        .studio_plate_delete(delete(1, Some(PlateDeleteContents::Delete), None, rev))
        .await
        .unwrap();
    assert_eq!(out["deleted_objects"], 1);
    assert_eq!(out["contents"], "delete");
    assert_eq!(out["plates"].as_array().unwrap().len(), 1);

    let mock = fixture();
    mock.add_object("obj-3", "x", 1);
    mock.add_instance("obj-1", "inst-b", 1);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_plate_delete(delete(1, Some(PlateDeleteContents::Delete), None, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::InvalidArgument);
    assert_eq!(e.details["reason"], "object_spans_plates");
    assert_eq!(count(&mock, "plate_delete"), 0);
    assert_eq!(plate_count(&t.studio_get_state().await.unwrap()), 2);
}

#[tokio::test]
async fn delete_returns_renumbered_plates_and_destination() {
    let mock = Arc::new(MockDriver::new());
    mock.set_plate_count(3);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    t.studio_move_to_plate(move_args("obj-1", None, 1, rev))
        .await
        .unwrap();
    let rev = revision(&t).await;
    let out = t
        .studio_plate_delete(delete(1, Some(PlateDeleteContents::Move), Some(2), rev))
        .await
        .unwrap();
    assert_eq!(out["deleted_plate_index"], 1);
    assert_eq!(out["moved_instances"], 1);
    assert_eq!(out["destination_plate_index"], 1);
    let plates = out["plates"].as_array().unwrap();
    assert_eq!(plates.len(), 2);
    assert_eq!(plates[1]["plate_index"], 1);
    assert_eq!(plates[0]["selected"], true);
    assert_eq!(plates[1]["locked"], false);
    assert_eq!(out["revision"].as_u64().unwrap(), revision(&t).await);
}

#[tokio::test]
async fn move_to_plate_defaults_the_only_instance_and_requires_instance_for_several() {
    let mock = fixture();
    mock.set_plate_count(2);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let out = t
        .studio_move_to_plate(move_args("obj-1", None, 1, rev))
        .await
        .unwrap();
    assert_eq!(out["moved"], true);
    assert_eq!(out["from_plate_index"], 0);
    assert_eq!(out["plate_index"], 1);
    assert_eq!(out["instance_id"], "inst-1");
    assert!(
        mock.instances_on_plate(1)
            .iter()
            .any(|(o, i)| o == "obj-1" && i == "inst-1")
    );

    mock.add_instance("obj-1", "inst-b", 0);
    let rev = revision(&t).await;
    let e = t
        .studio_move_to_plate(move_args("obj-1", None, 1, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::InstanceRequired);
    let out = t
        .studio_move_to_plate(move_args("obj-1", Some("inst-b"), 1, rev))
        .await
        .unwrap();
    assert_eq!(out["moved"], true);
}

#[tokio::test]
async fn move_to_the_same_plate_is_a_no_op_without_a_driver_call() {
    let mock = fixture();
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let out = t
        .studio_move_to_plate(move_args("obj-1", None, 0, rev))
        .await
        .unwrap();
    assert_eq!(out["moved"], false);
    assert_eq!(count(&mock, "plate_move_instance"), 0);
    assert_eq!(out["revision"].as_u64().unwrap(), rev);
}

#[tokio::test]
async fn move_to_locked_target_or_source_is_plate_locked_and_unknown_plate_is_plate_not_found() {
    let mock = fixture();
    mock.set_plate_count(2);
    mock.lock_plate(1, true);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_move_to_plate(move_args("obj-1", None, 1, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateLocked);
    assert_eq!(e.details["plate_index"], 1);

    mock.lock_plate(1, false);
    mock.lock_plate(0, true);
    let rev = revision(&t).await;
    let e = t
        .studio_move_to_plate(move_args("obj-1", None, 1, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateLocked);
    assert_eq!(e.details["plate_index"], 0);

    let e = t
        .studio_move_to_plate(move_args("obj-1", None, 7, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::PlateNotFound);
    let e = t
        .studio_move_to_plate(move_args("nope", None, 1, rev))
        .await
        .unwrap_err();
    assert_eq!(code(&e), ToolErrorCode::ObjectNotFound);
    assert_eq!(count(&mock, "plate_move_instance"), 0);
}

#[tokio::test]
async fn plate_tools_conflict_on_a_stale_expected_revision() {
    const WRITES: [&str; 4] = [
        "plate_create",
        "plate_rename",
        "plate_delete",
        "plate_move_instance",
    ];
    for which in 0..4 {
        let mock = fixture();
        mock.set_plate_count(2);
        let t = tools_with(&mock);
        let rev = revision(&t).await;
        mock.user_edit("layer_height", "0.3");
        let e = match which {
            0 => t.studio_plate_create(create(Some("n"), rev)).await,
            1 => t.studio_plate_rename(rename(0, "n", rev)).await,
            2 => t.studio_plate_delete(delete(1, None, None, rev)).await,
            _ => {
                t.studio_move_to_plate(move_args("obj-1", None, 1, rev))
                    .await
            }
        }
        .unwrap_err();
        assert_eq!(code(&e), ToolErrorCode::RevisionConflict, "tool {which}");
        for w in WRITES {
            assert_eq!(count(&mock, w), 0, "tool {which} wrote {w}");
        }
    }
}

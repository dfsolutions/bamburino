// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::time::Duration;

use bamburino_core::build_info;
use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::tools::{GetSettingsArgs, SearchArgs, TargetRef, ToolConfig, Tools};
use bamburino_core::validate::Scope;
use serde_json::Value;

fn tools_with(mock: &Arc<MockDriver>, process_running: bool) -> Tools {
    Tools::new(
        mock.clone(),
        Arc::new(FixedProcessProbe(process_running)),
        ToolConfig {
            allow_version_mismatch: false,
            poll_every: Duration::from_secs(60),
            idle_after: Duration::from_secs(60),
        },
    )
}

fn setting<'a>(v: &'a Value, key: &str) -> &'a Value {
    v["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap_or_else(|| panic!("{key} missing"))
}

#[test]
fn names_are_exact_and_twelve() {
    assert_eq!(
        Tools::NAMES,
        [
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
        ]
    );
}

#[tokio::test]
async fn capabilities_works_with_studio_closed() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    let v = tools_with(&mock, false).bamburino_capabilities().await;
    assert_eq!(v["driver"]["status"], "not_running");
    assert!(v["catalog"]["keys"].as_u64().unwrap() >= 590);
    assert_eq!(v["physical_tools"], false);
    assert_eq!(v["tools"].as_array().unwrap().len(), 12);
    assert_eq!(
        v["driver"]["expected_studio_version"],
        build_info::STUDIO_VERSION
    );
    assert_eq!(v["bamburino_version"], build_info::BAMBURINO_VERSION);
}

#[tokio::test]
async fn capabilities_reports_version_match_from_driver_capabilities() {
    let mock = Arc::new(MockDriver::new());
    let v = tools_with(&mock, true).bamburino_capabilities().await;
    assert_eq!(v["driver"]["status"], "ready");
    assert_eq!(v["driver"]["version_match"], true);
    assert_eq!(v["driver"]["app_version"], build_info::STUDIO_VERSION);
    // A different patch id only breaks the match once the patch set is frozen
    // (driver/upstream.toml); a different base version always does.
    let mock = Arc::new(MockDriver::new());
    mock.set_identity(build_info::STUDIO_VERSION, "some-other-patch");
    let v = tools_with(&mock, true).bamburino_capabilities().await;
    assert_eq!(v["driver"]["patch_id"], "some-other-patch");
    assert_eq!(
        v["driver"]["version_match"],
        build_info::EXPECTED_PATCH_ID.is_none()
    );

    let mock = Arc::new(MockDriver::new());
    mock.set_identity("01.00.00.00", "some-other-patch");
    let v = tools_with(&mock, true).bamburino_capabilities().await;
    assert_eq!(v["driver"]["status"], "ready");
    assert_eq!(v["driver"]["version_match"], false);
    assert_eq!(v["driver"]["app_version"], "01.00.00.00");
}

#[tokio::test]
async fn catalog_search_works_with_studio_closed() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    let t = tools_with(&mock, false);
    let v = t
        .catalog_search(SearchArgs {
            query: "infill density".into(),
            limit: Some(5),
        })
        .await;
    assert_eq!(v["query"], "infill density");
    let hits = v["hits"].as_array().unwrap();
    assert!(!hits.is_empty() && hits.len() <= 5);
    let top = hits
        .iter()
        .find(|h| h["key"] == "sparse_infill_density")
        .unwrap();
    assert_eq!(top["writable_by_driver"], true);
    let capped = t
        .catalog_search(SearchArgs {
            query: "speed".into(),
            limit: Some(500),
        })
        .await;
    assert!(capped["hits"].as_array().unwrap().len() <= 50);
}

#[tokio::test]
async fn get_state_with_studio_closed_is_structured_error() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    let e = tools_with(&mock, false)
        .studio_get_state()
        .await
        .unwrap_err();
    assert_eq!(e.to_json()["error"]["code"], "STUDIO_NOT_RUNNING");
}

#[tokio::test]
async fn get_state_reports_revision_and_presets() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock, true);
    let v = t.studio_get_state().await.unwrap();
    let rev = v["revision"].as_u64().unwrap();
    assert!(rev > 0);
    assert_eq!(rev, t.revision_state().current());
    assert_eq!(v["presets"]["printer"], "Mock Printer");
    assert_eq!(v["presets"]["filaments"][0], "Mock PLA");
    assert_eq!(v["plates"][0]["plate_id"], 0);
    assert_eq!(v["plates"][0]["selected"], true);
    assert_eq!(v["models"][0]["object_id"], "obj-1");
    assert_eq!(v["models"][0]["instances"][0]["plate_id"], 0);
    assert_eq!(v["busy"]["modal_open"], false);
    assert_eq!(v["slice"]["valid"], false);
    assert!(v["slice"]["slice_result_id"].is_null());
    // Unchanged state keeps the revision; a user edit moves it.
    assert_eq!(t.studio_get_state().await.unwrap()["revision"], rev);
    mock.user_edit("layer_height", "0.12");
    assert!(
        t.studio_get_state().await.unwrap()["revision"]
            .as_u64()
            .unwrap()
            > rev
    );
    mock.set_modal(true);
    assert_eq!(
        t.studio_get_state().await.unwrap()["busy"]["modal_open"],
        true
    );
}

#[tokio::test]
async fn list_models_carries_revision() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock, true);
    let v = t.studio_list_models().await.unwrap();
    assert_eq!(v["models"][0]["name"], "cube");
    assert_eq!(
        v["revision"].as_u64().unwrap(),
        t.revision_state().current()
    );
}

#[tokio::test]
async fn get_settings_project_marks_writable_by_driver() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock, true);
    let v = t
        .studio_get_settings(GetSettingsArgs {
            scope: Scope::Project,
            target: None,
        })
        .await
        .unwrap();
    assert_eq!(v["scope"], "project");
    assert!(v["revision"].as_u64().unwrap() > 0);
    let infill = setting(&v, "sparse_infill_density");
    assert_eq!(infill["writable_by_driver"], true);
    assert_eq!(infill["origin"], "project");
    assert_eq!(infill["type"], "Percent");
    assert_eq!(setting(&v, "ironing_type")["writable_by_driver"], false);
    assert!(setting(&v, "ironing_type")["enum"].is_array());
}

#[tokio::test]
async fn get_settings_object_reports_origin_object_for_overrides() {
    let mock = Arc::new(MockDriver::new());
    mock.user_object_override("layer_height", "0.1");
    let t = tools_with(&mock, true);
    let v = t
        .studio_get_settings(GetSettingsArgs {
            scope: Scope::Object,
            target: Some(TargetRef {
                plate_id: None,
                object_id: Some("obj-1".into()),
            }),
        })
        .await
        .unwrap();
    assert_eq!(v["target"]["object_id"], "obj-1");
    let lh = setting(&v, "layer_height");
    assert_eq!(lh["value"], "0.1");
    assert_eq!(lh["origin"], "object");
    assert_eq!(setting(&v, "wall_loops")["origin"], "project");
}

#[tokio::test]
async fn get_settings_plate_works_and_unknown_targets_fail() {
    let mock = Arc::new(MockDriver::new());
    mock.user_plate_override("brim_width", "5");
    let t = tools_with(&mock, true);
    let v = t
        .studio_get_settings(GetSettingsArgs {
            scope: Scope::Plate,
            target: Some(TargetRef {
                plate_id: Some(0),
                object_id: None,
            }),
        })
        .await
        .unwrap();
    assert_eq!(setting(&v, "brim_width")["origin"], "plate");

    let e = t
        .studio_get_settings(GetSettingsArgs {
            scope: Scope::Plate,
            target: Some(TargetRef {
                plate_id: Some(7),
                object_id: None,
            }),
        })
        .await
        .unwrap_err();
    assert_eq!(e.to_json()["error"]["code"], "PLATE_NOT_FOUND");
    let e = t
        .studio_get_settings(GetSettingsArgs {
            scope: Scope::Object,
            target: Some(TargetRef {
                plate_id: None,
                object_id: Some("nope".into()),
            }),
        })
        .await
        .unwrap_err();
    assert_eq!(e.to_json()["error"]["code"], "OBJECT_NOT_FOUND");
}

#[tokio::test]
async fn get_settings_object_requires_object_id() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock, true);
    for target in [
        None,
        Some(TargetRef {
            plate_id: Some(0),
            object_id: None,
        }),
    ] {
        let e = t
            .studio_get_settings(GetSettingsArgs {
                scope: Scope::Object,
                target,
            })
            .await
            .unwrap_err();
        assert_eq!(e.to_json()["error"]["code"], "INVALID_ARGUMENT");
    }
    let e = t
        .studio_get_settings(GetSettingsArgs {
            scope: Scope::Plate,
            target: None,
        })
        .await
        .unwrap_err();
    assert_eq!(e.to_json()["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn args_reject_unknown_fields() {
    assert!(serde_json::from_str::<SearchArgs>(r#"{"query":"a","extra":1}"#).is_err());
    assert!(serde_json::from_str::<GetSettingsArgs>(r#"{"scope":"project","x":1}"#).is_err());
    assert!(serde_json::from_str::<TargetRef>(r#"{"plate_id":1,"y":1}"#).is_err());
}

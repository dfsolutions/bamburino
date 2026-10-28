// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::errors::{ToolError, ToolErrorCode};
use bamburino_core::tools::{
    SupportKind, SupportSettings, SupportStyle, SupportTarget, SupportsArgs, ToolConfig, Tools,
    Transform, TransformArgs, TransformMode, compose_rotation_deg,
};
use bamburino_core::validate::Scope;
use serde_json::{Value, json};

fn tools_with(mock: &Arc<MockDriver>) -> Tools {
    tools_with_process(mock, true)
}

fn tools_with_process(mock: &Arc<MockDriver>, studio_running: bool) -> Tools {
    Tools::new(
        mock.clone(),
        Arc::new(FixedProcessProbe(studio_running)),
        ToolConfig {
            allow_version_mismatch: false,
            poll_every: Duration::from_secs(60),
            idle_after: Duration::from_secs(60),
        },
        std::env::temp_dir().join("bamburino-tests-unused"),
    )
}

async fn revision(t: &Tools) -> u64 {
    t.studio_get_state().await.unwrap()["revision"]
        .as_u64()
        .unwrap()
}

fn tf(rot: Option<[f64; 3]>, tr: Option<[f64; 3]>, sc: Option<[f64; 3]>) -> Transform {
    Transform {
        rotate_deg: rot,
        translate_mm: tr,
        scale: sc,
        mode: None,
    }
}

fn targs(object: &str, transform: Transform, rev: u64) -> TransformArgs {
    TransformArgs {
        object_id: object.to_string(),
        instance_id: None,
        transform,
        expected_revision: rev,
    }
}

fn rot_z(deg: f64) -> Transform {
    tf(Some([0.0, 0.0, deg]), None, None)
}

async fn rotation_of(t: &Tools) -> Vec<f64> {
    let m = t.studio_list_models().await.unwrap();
    m["models"][0]["instances"][0]["rotation_deg"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect()
}

fn close(a: &[f64], b: &[f64]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

#[tokio::test]
async fn rotate_z_90_relative() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let r = t
        .studio_transform_model(targs("obj-1", rot_z(90.0), rev))
        .await
        .unwrap();
    assert_eq!(r["object_id"], "obj-1");
    assert_eq!(r["instance_id"], "inst-1");
    assert_eq!(r["before"]["rotation_deg"], json!([0.0, 0.0, 0.0]));
    assert_eq!(r["after"]["rotation_deg"], json!([0.0, 0.0, 90.0]));
    assert_eq!(r["reread"]["rotation_deg"], json!([0.0, 0.0, 90.0]));
    assert_ne!(r["revision"].as_u64().unwrap(), rev);
    assert_eq!(rotation_of(&t).await, vec![0.0, 0.0, 90.0]);
}

#[test]
fn compose_two_z90_is_z180() {
    let r = compose_rotation_deg([0.0, 0.0, 90.0], [0.0, 0.0, 90.0]);
    assert!(close(&r, &[0.0, 0.0, 180.0]), "{r:?}");
}

type Mat = [[f64; 3]; 3];

fn mm(a: &Mat, b: &Mat) -> Mat {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

#[test]
fn compose_x90_then_z90_matches_matrix_product() {
    // Rx(90) then Rz(90) about the world axes: M = Rz * Rx.
    let rx = [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]];
    let rz = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let expected = mm(&rz, &rx);
    let e = compose_rotation_deg([90.0, 0.0, 0.0], [0.0, 0.0, 90.0]);
    // Rebuild the matrix of the result as Rz(e2) * Ry(e1) * Rx(e0).
    let [x, y, z] = e.map(f64::to_radians);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let got = mm(&rz, &mm(&ry, &rx));
    for i in 0..3 {
        for j in 0..3 {
            assert!(
                (got[i][j] - expected[i][j]).abs() < 1e-9,
                "{e:?} {got:?} {expected:?}"
            );
        }
    }
}

#[tokio::test]
async fn retry_with_same_revision_conflicts() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    t.studio_transform_model(targs("obj-1", rot_z(90.0), rev))
        .await
        .unwrap();
    let e = t
        .studio_transform_model(targs("obj-1", rot_z(90.0), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(e.details["reason"], "revision_mismatch");
    assert_eq!(rotation_of(&t).await, vec![0.0, 0.0, 90.0]);
}

#[tokio::test]
async fn conflict_after_user_edit_writes_nothing() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    mock.user_edit("layer_height", "0.3");
    let e = t
        .studio_transform_model(targs("obj-1", rot_z(90.0), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(rotation_of(&t).await, vec![0.0, 0.0, 0.0]);
}

#[tokio::test]
async fn busy_on_modal() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    mock.set_modal(true);
    let e = t
        .studio_transform_model(targs("obj-1", rot_z(90.0), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
    mock.set_modal(false);
    assert_eq!(rotation_of(&t).await, vec![0.0, 0.0, 0.0]);
}

#[tokio::test]
async fn unknown_object_returns_object_not_found() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_transform_model(targs("nope", rot_z(90.0), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::ObjectNotFound);
    assert_eq!(e.details["models"][0]["object_id"], "obj-1");
    let mut a = targs("obj-1", rot_z(90.0), rev);
    a.instance_id = Some("nope".into());
    let e = t.studio_transform_model(a).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::ObjectNotFound);
    assert_eq!(rotation_of(&t).await, vec![0.0, 0.0, 0.0]);
}

#[tokio::test]
async fn empty_transform_is_invalid_argument() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let e = t
        .studio_transform_model(targs("obj-1", tf(None, None, None), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
    let e = t
        .studio_transform_model(targs("obj-1", tf(None, None, Some([0.0, 1.0, 1.0])), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
}

#[tokio::test]
async fn translate_and_scale_relative_then_absolute() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let r = t
        .studio_transform_model(targs(
            "obj-1",
            tf(None, Some([10.0, -5.0, 0.0]), Some([2.0, 2.0, 2.0])),
            rev,
        ))
        .await
        .unwrap();
    assert_eq!(r["after"]["position_mm"], json!([138.0, 123.0, 10.0]));
    assert_eq!(r["after"]["scale"], json!([2.0, 2.0, 2.0]));
    let abs = Transform {
        rotate_deg: None,
        translate_mm: Some([50.0, 60.0, 10.0]),
        scale: Some([1.0, 1.0, 1.0]),
        mode: Some(TransformMode::Absolute),
    };
    let rev = r["revision"].as_u64().unwrap();
    let r = t
        .studio_transform_model(targs("obj-1", abs, rev))
        .await
        .unwrap();
    assert_eq!(r["after"]["position_mm"], json!([50.0, 60.0, 10.0]));
    assert_eq!(r["after"]["scale"], json!([1.0, 1.0, 1.0]));
}

#[tokio::test]
async fn snapshot_rule_holds_for_transform() {
    let mock = Arc::new(MockDriver::new());
    mock.set_undo_snapshot_supported(true);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let r = t
        .studio_transform_model(targs("obj-1", rot_z(90.0), rev))
        .await
        .unwrap();
    assert_eq!(r["after"]["rotation_deg"], json!([0.0, 0.0, 90.0]));
}

fn supports(target: SupportTarget, settings: SupportSettings, rev: u64) -> SupportsArgs {
    SupportsArgs {
        target,
        settings,
        expected_revision: rev,
    }
}

fn tree() -> SupportSettings {
    SupportSettings {
        enabled: true,
        r#type: Some(SupportKind::Tree),
        style: None,
        threshold_angle: None,
        on_build_plate_only: None,
    }
}

fn applied(r: &Value, key: &str) -> Vec<String> {
    r["applied"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["key"] == key)
        .map(|a| a["applied"].as_str().unwrap().to_string())
        .collect()
}

fn err_msg(e: &ToolError) -> String {
    format!("{e:?}")
}

#[tokio::test]
async fn supports_tree_project() {
    let mock = Arc::new(MockDriver::new());
    mock.set_undo_snapshot_supported(true);
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let target = SupportTarget {
        scope: Scope::Project,
        plate_id: None,
        object_id: None,
    };
    let r = t
        .studio_set_supports(supports(target, tree(), rev))
        .await
        .unwrap_or_else(|e| panic!("{}", err_msg(&e)));
    assert_eq!(applied(&r, "enable_support"), vec!["1"]);
    assert_eq!(applied(&r, "support_type"), vec!["tree(auto)"]);
    assert!(r["revision"].as_u64().is_some());
}

#[tokio::test]
async fn supports_manual_normal_with_options() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let target = SupportTarget {
        scope: Scope::Project,
        plate_id: None,
        object_id: None,
    };
    let s = SupportSettings {
        enabled: true,
        r#type: Some(SupportKind::Normal),
        style: Some(SupportStyle::Manual),
        threshold_angle: Some(40),
        on_build_plate_only: Some(true),
    };
    let r = t
        .studio_set_supports(supports(target, s, rev))
        .await
        .unwrap_or_else(|e| panic!("{}", err_msg(&e)));
    assert_eq!(applied(&r, "support_type"), vec!["normal(manual)"]);
    assert_eq!(applied(&r, "support_threshold_angle"), vec!["40"]);
    assert_eq!(applied(&r, "support_on_build_plate_only"), vec!["1"]);
}

#[tokio::test]
async fn supports_object_scope_uses_override() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let target = SupportTarget {
        scope: Scope::Object,
        plate_id: None,
        object_id: Some("obj-1".into()),
    };
    let r = t
        .studio_set_supports(supports(target, tree(), rev))
        .await
        .unwrap_or_else(|e| panic!("{}", err_msg(&e)));
    assert_eq!(applied(&r, "support_type"), vec!["tree(auto)"]);
    assert_eq!(r["applied"][0]["scope"], "object");
    let calls = mock.calls().join(",");
    assert!(calls.contains("settings_override"), "{calls}");
    assert!(!calls.contains("settings_update"), "{calls}");
}

#[tokio::test]
async fn supports_plate_scope_is_a_plate_override() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let target = SupportTarget {
        scope: Scope::Plate,
        plate_id: Some(0),
        object_id: None,
    };
    let r = t
        .studio_set_supports(supports(target, tree(), rev))
        .await
        .unwrap_or_else(|e| panic!("{}", err_msg(&e)));
    assert_eq!(applied(&r, "enable_support"), vec!["1"]);
    assert_eq!(applied(&r, "support_type"), vec!["tree(auto)"]);
    // Only plate-scoped changes, one per key: no per-object expansion.
    let entries = r["applied"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{r}");
    for a in entries {
        assert_eq!(a["scope"], "plate", "{a}");
        assert_eq!(a["target"]["plate_id"], 0, "{a}");
        assert!(a["target"]["object_id"].is_null(), "{a}");
    }
    let calls = mock.calls().join(",");
    assert!(calls.contains("settings_override"), "{calls}");
    assert!(!calls.contains("settings_update"), "{calls}");
}

#[tokio::test]
async fn supports_plate_scope_with_studio_closed_is_not_running() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with_process(&mock, false);
    mock.set_unreachable(true);
    let target = SupportTarget {
        scope: Scope::Plate,
        plate_id: Some(0),
        object_id: None,
    };
    let e = t
        .studio_set_supports(supports(target, tree(), 1))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::StudioNotRunning, "{}", err_msg(&e));
}

#[tokio::test]
async fn supports_plate_scope_on_a_missing_plate_writes_nothing() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let target = SupportTarget {
        scope: Scope::Plate,
        plate_id: Some(7),
        object_id: None,
    };
    let e = t
        .studio_set_supports(supports(target, tree(), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PlateNotFound, "{}", err_msg(&e));
    let calls = mock.calls().join(",");
    assert!(!calls.contains("settings_override"), "{calls}");
}

#[tokio::test]
async fn supports_conflict_and_busy() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let rev = revision(&t).await;
    let target = || SupportTarget {
        scope: Scope::Project,
        plate_id: None,
        object_id: None,
    };
    mock.user_edit("layer_height", "0.3");
    let e = t
        .studio_set_supports(supports(target(), tree(), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    let rev = revision(&t).await;
    mock.set_modal(true);
    let e = t
        .studio_set_supports(supports(target(), tree(), rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
}

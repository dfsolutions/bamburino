// SPDX-License-Identifier: AGPL-3.0-or-later
//! `studio_save_project` and `studio_export_plate` against the MockDriver,
//! with a real path allowlist on a temp directory.
#![cfg(all(feature = "mock", windows))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::DriverError;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::driver::tool::DriverTool;
use bamburino_core::errors::{ToolError, ToolErrorCode};
use bamburino_core::pathguard::PathGuard;
use bamburino_core::tools::{
    Change, ExportPlateArgs, JobArgs, SaveProjectArgs, SetSettingsArgs, SliceArgs, ToolConfig,
    Tools,
};
use serde_json::json;
use sha2::{Digest, Sha256};

/// `<tmp>\allowed` is the only allowed directory; `<tmp>\outside` is next to it.
struct Env {
    _dir: tempfile::TempDir,
    allowed: PathBuf,
    outside: PathBuf,
    mock: Arc<MockDriver>,
    tools: Tools,
}

fn tools_for(mock: &Arc<MockDriver>) -> Tools {
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

fn env_with(mock: Arc<MockDriver>, guard: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
    let allowed = base.join("allowed");
    let outside = base.join("outside");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let tools = tools_for(&mock);
    if guard {
        let (g, excluded) = PathGuard::new(vec![allowed.clone()]);
        assert!(excluded.is_empty(), "{excluded:?}");
        tools.set_path_guard(g);
    }
    Env {
        _dir: dir,
        allowed,
        outside,
        mock,
        tools,
    }
}

fn env() -> Env {
    env_with(Arc::new(MockDriver::new()), true)
}

/// Two plates (C2): the default cube on plate 0, `obj-2` on plate 1.
fn env_two_plates() -> Env {
    let m = Arc::new(MockDriver::new());
    m.set_plate_count(2);
    m.add_object("obj-2", "second", 1);
    env_with(m, true)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn save(path: &Path, overwrite: Option<bool>) -> SaveProjectArgs {
    SaveProjectArgs {
        path: s(path),
        overwrite,
    }
}

fn export(plate_index: u32, path: &Path, overwrite: Option<bool>) -> ExportPlateArgs {
    ExportPlateArgs {
        plate_index,
        path: s(path),
        overwrite,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn writes(mock: &MockDriver) -> Vec<String> {
    mock.calls()
        .into_iter()
        .filter(|c| c == "project_save" || c == "export_sliced_3mf")
        .collect()
}

async fn revision(t: &Tools) -> u64 {
    t.studio_get_state().await.unwrap()["revision"]
        .as_u64()
        .unwrap()
}

/// Slices `plate` to completion (selecting it if needed).
async fn slice_plate(t: &Tools, plate: u32) {
    let id = t
        .studio_slice(SliceArgs {
            plate_id: Some(plate),
        })
        .await
        .unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..5 {
        let st = t
            .studio_slice_status(JobArgs { job_id: id.clone() })
            .await
            .unwrap();
        if st["status"] == "succeeded" {
            return;
        }
    }
    panic!("slice of plate {plate} did not succeed");
}

fn set_wall_loops(n: u32, rev: u64) -> SetSettingsArgs {
    SetSettingsArgs {
        changes: vec![Change {
            key: "wall_loops".to_string(),
            value: json!(n),
            scope: None,
            target: None,
        }],
        expected_revision: rev,
        dry_run: None,
    }
}

fn assert_path_not_allowed(e: &ToolError, reason: &str, path: &Path) {
    assert_eq!(e.code, ToolErrorCode::PathNotAllowed, "{e:?}");
    assert_eq!(e.http_status, 403);
    assert_eq!(e.details["reason"], reason, "{e:?}");
    assert!(
        !e.message.contains(&s(path)) && !e.message.contains("outside\\"),
        "the message must not echo the path: {}",
        e.message
    );
}

#[tokio::test]
async fn path_outside_allowed_dirs_is_rejected_before_any_driver_call() {
    let env = env();
    let out3mf = env.outside.join("p.3mf");
    let outsliced = env.outside.join("p.gcode.3mf");

    let e = env
        .tools
        .studio_save_project(save(&out3mf, None))
        .await
        .unwrap_err();
    assert_path_not_allowed(&e, "outside_roots", &out3mf);
    let e = env
        .tools
        .studio_export_plate(export(0, &outsliced, None))
        .await
        .unwrap_err();
    assert_path_not_allowed(&e, "outside_roots", &outsliced);

    // Lexical violations inside the root keep their own reason.
    let traversal = format!(r"{}\..\outside\p.3mf", s(&env.allowed));
    let e = env
        .tools
        .studio_save_project(SaveProjectArgs {
            path: traversal,
            overwrite: None,
        })
        .await
        .unwrap_err();
    assert_eq!(e.details["reason"], "traversal");
    let missing_dir = env.allowed.join("nope").join("p.gcode.3mf");
    let e = env
        .tools
        .studio_export_plate(export(0, &missing_dir, None))
        .await
        .unwrap_err();
    assert_path_not_allowed(&e, "not_found", &missing_dir);

    assert!(env.mock.calls().is_empty(), "{:?}", env.mock.calls());
    assert!(!out3mf.exists() && !outsliced.exists());

    // No guard installed: nothing is allowed, still no driver call.
    let bare = env_with(Arc::new(MockDriver::new()), false);
    let inside = bare.allowed.join("p.3mf");
    let e = bare
        .tools
        .studio_save_project(save(&inside, None))
        .await
        .unwrap_err();
    assert_path_not_allowed(&e, "outside_roots", &inside);
    let e = bare
        .tools
        .studio_export_plate(export(0, &bare.allowed.join("p.gcode.3mf"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PathNotAllowed);
    assert!(bare.mock.calls().is_empty(), "{:?}", bare.mock.calls());
    assert!(!inside.exists());
}

#[tokio::test]
async fn wrong_suffix_is_invalid_argument_and_case_is_ignored() {
    let env = env();
    for bad in ["x.gcode", "x.3mf", "x.stl"] {
        let e = env
            .tools
            .studio_export_plate(export(0, &env.allowed.join(bad), None))
            .await
            .unwrap_err();
        assert_eq!(e.code, ToolErrorCode::InvalidArgument, "{bad}");
        assert_eq!(e.details["reason"], "wrong_suffix");
    }
    for bad in ["x.stl", "x.gcode.3mf"] {
        let e = env
            .tools
            .studio_save_project(save(&env.allowed.join(bad), None))
            .await
            .unwrap_err();
        assert_eq!(e.code, ToolErrorCode::InvalidArgument, "{bad}");
    }
    // The suffix check comes first, even for a path outside the roots.
    let e = env
        .tools
        .studio_export_plate(export(0, &env.outside.join("x.gcode"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
    assert!(env.mock.calls().is_empty(), "{:?}", env.mock.calls());

    slice_plate(&env.tools, 0).await;
    let upper = env.allowed.join("X.GCODE.3MF");
    let r = env
        .tools
        .studio_export_plate(export(0, &upper, None))
        .await
        .unwrap();
    assert_eq!(r["path"], s(&upper));
    assert!(upper.exists());

    let r = env
        .tools
        .studio_save_project(save(&env.allowed.join("A.3MF"), None))
        .await
        .unwrap();
    let sent = env.allowed.join("A.3mf");
    assert_eq!(r["path"], s(&sent));
    let state = env.tools.studio_get_state().await.unwrap();
    assert_eq!(state["project"]["path"], s(&sent), "the driver got .3mf");
}

#[tokio::test]
async fn overwrite_is_never_sent_true_by_default() {
    let env = env();
    slice_plate(&env.tools, 0).await;
    env.tools
        .studio_export_plate(export(0, &env.allowed.join("a.gcode.3mf"), None))
        .await
        .unwrap();
    env.tools
        .studio_save_project(save(&env.allowed.join("a.3mf"), None))
        .await
        .unwrap();
    assert_eq!(env.mock.file_overwrite_flags(), vec![false, false]);

    // Explicit true is passed through (new files: the flag is still sent).
    slice_plate(&env.tools, 0).await;
    env.tools
        .studio_export_plate(export(0, &env.allowed.join("b.gcode.3mf"), Some(true)))
        .await
        .unwrap();
    env.tools
        .studio_save_project(save(&env.allowed.join("b.3mf"), Some(true)))
        .await
        .unwrap();
    // Explicit false stays false.
    env.tools
        .studio_save_project(save(&env.allowed.join("c.3mf"), Some(false)))
        .await
        .unwrap();
    assert_eq!(
        env.mock.file_overwrite_flags(),
        vec![false, false, true, true, false]
    );
}

#[tokio::test]
async fn existing_file_without_overwrite_is_file_exists_and_makes_no_driver_call() {
    let env = env();
    let project = env.allowed.join("p.3mf");
    let sliced = env.allowed.join("p.gcode.3mf");
    std::fs::write(&project, b"old project").unwrap();
    std::fs::write(&sliced, b"old sliced").unwrap();

    for overwrite in [None, Some(false)] {
        let e = env
            .tools
            .studio_save_project(save(&project, overwrite))
            .await
            .unwrap_err();
        assert_eq!(e.code, ToolErrorCode::FileExists, "{e:?}");
        assert_eq!(e.http_status, 422);
        let e = env
            .tools
            .studio_export_plate(export(0, &sliced, overwrite))
            .await
            .unwrap_err();
        assert_eq!(e.code, ToolErrorCode::FileExists, "{e:?}");
    }
    // Same file through another case of the suffix.
    let e = env
        .tools
        .studio_save_project(save(&env.allowed.join("P.3MF"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::FileExists);
    assert!(env.mock.calls().is_empty(), "{:?}", env.mock.calls());
    assert_eq!(std::fs::read(&project).unwrap(), b"old project");

    // With overwrite: replaced, and the hash is the new content's.
    let r = env
        .tools
        .studio_save_project(save(&project, Some(true)))
        .await
        .unwrap();
    let now = std::fs::read(&project).unwrap();
    assert_ne!(now, b"old project");
    assert_eq!(r["sha256"], sha256_hex(&now));
    slice_plate(&env.tools, 0).await;
    let r = env
        .tools
        .studio_export_plate(export(0, &sliced, Some(true)))
        .await
        .unwrap();
    let now = std::fs::read(&sliced).unwrap();
    assert_ne!(now, b"old sliced");
    assert_eq!(r["sha256"], sha256_hex(&now));
    assert_eq!(env.mock.file_overwrite_flags(), vec![true, true]);
}

#[tokio::test]
async fn driver_destination_exists_message_maps_to_file_exists() {
    let env = env();
    // C3: a current slice first, so the call reaches the driver.
    slice_plate(&env.tools, 0).await;
    env.mock.fail_tool(
        DriverTool::ExportSliced3mf,
        DriverError::InvalidParams {
            message: "Destination exists; explicitly set overwrite=true".to_string(),
        },
        1,
    );
    let e = env
        .tools
        .studio_export_plate(export(0, &env.allowed.join("race.gcode.3mf"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::FileExists, "{e:?}");
    assert!(env.mock.calls().contains(&"export_sliced_3mf".to_string()));

    env.mock.fail_tool(
        DriverTool::ProjectSave,
        DriverError::InvalidParams {
            message: "Destination exists; set overwrite=true".to_string(),
        },
        1,
    );
    let e = env
        .tools
        .studio_save_project(save(&env.allowed.join("race.3mf"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::FileExists, "{e:?}");

    // The driver's own directory check (it runs as the user) maps to the
    // guard's reason.
    env.mock.fail_tool(
        DriverTool::ProjectSave,
        DriverError::InvalidParams {
            message: "Destination directory does not exist".to_string(),
        },
        1,
    );
    let e = env
        .tools
        .studio_save_project(save(&env.allowed.join("gone.3mf"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PathNotAllowed);
    assert_eq!(e.details["reason"], "not_found");
}

#[tokio::test]
async fn save_project_writes_a_file_and_reports_bytes_and_sha256() {
    let env = env();
    let path = env.allowed.join("project.3mf");
    let r = env
        .tools
        .studio_save_project(save(&path, None))
        .await
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(r["path"], s(&path));
    assert_eq!(r["bytes"], bytes.len() as u64);
    assert_eq!(r["sha256"], sha256_hex(&bytes));
    assert_eq!(r["sha256"].as_str().unwrap().len(), 64);
    assert!(r.get("hash_unavailable_reason").is_none(), "{r}");
    let state = env.tools.studio_get_state().await.unwrap();
    assert_eq!(state["project"]["path"], s(&path));
    assert_eq!(state["project"]["dirty"], false);
}

#[tokio::test]
async fn revision_returned_by_save_is_usable_for_the_next_mutation() {
    let env = env();
    let rev = revision(&env.tools).await;
    let r = env
        .tools
        .studio_save_project(save(&env.allowed.join("p.3mf"), None))
        .await
        .unwrap();
    let saved_rev = r["revision"].as_u64().unwrap();
    assert_ne!(saved_rev, rev, "saving changes the project path");
    assert_eq!(saved_rev, revision(&env.tools).await);
    let out = env
        .tools
        .studio_set_settings(set_wall_loops(5, saved_rev))
        .await;
    assert!(out.is_ok(), "{out:?}");
    // The pre-save revision is stale.
    let e = env
        .tools
        .studio_set_settings(set_wall_loops(6, rev))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
}

#[tokio::test]
async fn export_requires_a_current_slice_of_that_plate() {
    let env = env_two_plates();
    let p0 = env.allowed.join("p0.gcode.3mf");
    let p1 = env.allowed.join("p1.gcode.3mf");
    let check = |e: ToolError, plate: u32, selected: u32| {
        assert_eq!(e.code, ToolErrorCode::NotSliced, "{e:?}");
        assert_eq!(e.http_status, 422);
        assert_eq!(e.details["plate_index"], plate);
        assert_eq!(e.details["selected_plate_index"], selected);
        assert_eq!(
            e.details["hint"],
            format!("run studio_slice with plate_id {plate}, then studio_export_plate")
        );
    };

    // Never sliced.
    let e = env
        .tools
        .studio_export_plate(export(0, &p0, None))
        .await
        .unwrap_err();
    check(e, 0, 0);

    // Sliced plate 0, export plate 1 (exists, not selected, not sliced).
    slice_plate(&env.tools, 0).await;
    let e = env
        .tools
        .studio_export_plate(export(1, &p1, None))
        .await
        .unwrap_err();
    check(e, 1, 0);

    // Sliced plate 0, then an edit: the slice is stale.
    let rev = revision(&env.tools).await;
    env.tools
        .studio_set_settings(set_wall_loops(5, rev))
        .await
        .unwrap();
    let e = env
        .tools
        .studio_export_plate(export(0, &p0, None))
        .await
        .unwrap_err();
    check(e, 0, 0);

    assert!(
        !env.mock.calls().contains(&"export_sliced_3mf".to_string()),
        "{:?}",
        env.mock.calls()
    );
    assert!(!p0.exists() && !p1.exists());

    // Out of range: PLATE_NOT_FOUND, no driver write.
    let e = env
        .tools
        .studio_export_plate(export(5, &p0, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PlateNotFound);
    assert_eq!(e.details["plate_count"], 2);

    // The driver reporting a stale slice (edit in the gap) is NOT_SLICED too.
    slice_plate(&env.tools, 0).await;
    env.mock.fail_tool(
        DriverTool::ExportSliced3mf,
        DriverError::StaleReference {
            message: "Selected plate no longer has the requested completed slice".to_string(),
        },
        1,
    );
    let e = env
        .tools
        .studio_export_plate(export(0, &p0, None))
        .await
        .unwrap_err();
    check(e, 0, 0);
    assert!(!p0.exists());
    assert_eq!(writes(&env.mock), vec!["export_sliced_3mf".to_string()]);
}

#[tokio::test]
async fn export_after_slice_returns_path_sha256_bytes_and_slice_result_id() {
    let env = env_two_plates();
    // Plate 1 (C2): slicing it selects it.
    slice_plate(&env.tools, 1).await;
    let state = env.tools.studio_get_state().await.unwrap();
    let result_id = state["slice"]["slice_result_id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = env.allowed.join("plate2.gcode.3mf");
    let r = env
        .tools
        .studio_export_plate(export(1, &path, None))
        .await
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(r["path"], s(&path));
    assert_eq!(r["plate_index"], 1);
    assert_eq!(r["slice_result_id"], result_id);
    assert_eq!(r["bytes"], bytes.len() as u64);
    assert_eq!(r["sha256"], sha256_hex(&bytes));
    assert!(r.get("hash_unavailable_reason").is_none(), "{r}");
    assert!(r["revision"].as_u64().is_some());
    // Exporting does not invalidate the slice: a second export works.
    let again = env.allowed.join("plate2-copy.gcode.3mf");
    env.tools
        .studio_export_plate(export(1, &again, None))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&again).unwrap(), bytes);
}

#[tokio::test]
async fn export_succeeds_with_null_sha256_when_the_file_is_unreadable() {
    let env = env();
    env.mock.set_write_files(false);
    slice_plate(&env.tools, 0).await;
    let path = env.allowed.join("ghost.gcode.3mf");
    let r = env
        .tools
        .studio_export_plate(export(0, &path, None))
        .await
        .unwrap();
    assert!(!path.exists());
    assert!(r["sha256"].is_null(), "{r}");
    let why = r["hash_unavailable_reason"].as_str().unwrap();
    assert!(!why.is_empty());
    assert!(r["bytes"].as_u64().unwrap() > 0, "the driver's count: {r}");
    let state = env.tools.studio_get_state().await.unwrap();
    assert_eq!(r["slice_result_id"], state["slice"]["slice_result_id"]);

    // Save: the length comes from the file, so it is null too.
    let r = env
        .tools
        .studio_save_project(save(&env.allowed.join("ghost.3mf"), None))
        .await
        .unwrap();
    assert!(r["sha256"].is_null() && r["bytes"].is_null(), "{r}");
    assert!(!r["hash_unavailable_reason"].as_str().unwrap().is_empty());
    assert!(r["revision"].as_u64().is_some());
}

#[tokio::test]
async fn file_tools_are_busy_when_a_modal_is_open_and_make_no_driver_write() {
    let env = env();
    slice_plate(&env.tools, 0).await;
    env.mock.set_modal(true);
    let e = env
        .tools
        .studio_save_project(save(&env.allowed.join("m.3mf"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy, "{e:?}");
    assert_eq!(e.details["reason"], "modal_open");
    let e = env
        .tools
        .studio_export_plate(export(0, &env.allowed.join("m.gcode.3mf"), None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy, "{e:?}");
    assert!(writes(&env.mock).is_empty(), "{:?}", env.mock.calls());
    assert!(env.mock.file_overwrite_flags().is_empty());
    assert!(!env.allowed.join("m.3mf").exists());
    assert!(!env.allowed.join("m.gcode.3mf").exists());
}

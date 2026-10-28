// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::driver::{DriverError, DriverTool, FilamentUsage};
use bamburino_core::errors::ToolErrorCode;
use bamburino_core::tools::{JobArgs, SliceArgs, SummaryArgs, ToolConfig, Tools};
use serde_json::Value;

fn tools_with(mock: &Arc<MockDriver>) -> Tools {
    tools_in(mock, test_dir())
}

fn tools_in(mock: &Arc<MockDriver>, dir: PathBuf) -> Tools {
    Tools::new(
        mock.clone(),
        Arc::new(FixedProcessProbe(true)),
        ToolConfig {
            allow_version_mismatch: false,
            poll_every: Duration::from_secs(60),
            idle_after: Duration::from_secs(60),
        },
        dir,
    )
}

/// A fresh export directory per test (the production one is created by the
/// installer; the service does not create it).
fn test_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join("bamburino-tests").join(format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn slice_args(plate: Option<u32>) -> SliceArgs {
    SliceArgs { plate_id: plate }
}

fn job(id: &str) -> JobArgs {
    JobArgs {
        job_id: id.to_string(),
    }
}

fn summary_args(plate: Option<u32>) -> SummaryArgs {
    SummaryArgs { plate_id: plate }
}

fn two_filaments() -> Vec<FilamentUsage> {
    vec![
        FilamentUsage {
            index: 0,
            grams: Some(12.34),
            meters: Some(4.5),
        },
        FilamentUsage {
            index: 1,
            grams: Some(0.5),
            meters: Some(0.2),
        },
    ]
}

async fn start(t: &Tools) -> String {
    t.studio_slice(slice_args(None)).await.unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Slices the selected plate to completion and returns the job id.
async fn slice_to_done(t: &Tools) -> String {
    let id = start(t).await;
    for _ in 0..5 {
        let s = t.studio_slice_status(job(&id)).await.unwrap();
        if s["status"] == "succeeded" {
            return id;
        }
    }
    panic!("job did not succeed");
}

fn count(mock: &MockDriver, name: &str) -> usize {
    mock.calls().iter().filter(|c| *c == name).count()
}

#[tokio::test]
async fn slice_selected_plate_returns_job() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let r = t.studio_slice(slice_args(None)).await.unwrap();
    assert_eq!(r["job_id"], "mock-job-1");
    assert_eq!(r["plate_id"], 0);
    assert_eq!(r["status"], "running");
}

#[tokio::test]
async fn slice_status_polls_to_succeeded() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let id = start(&t).await;
    let first = t.studio_slice_status(job(&id)).await.unwrap();
    assert_eq!(first["status"], "running");
    assert_eq!(first["progress_percent"], 50);
    assert_eq!(first["plate_id"], 0);
    assert_eq!(first["slice_result_id"], Value::Null);
    let second = t.studio_slice_status(job(&id)).await.unwrap();
    assert_eq!(second["status"], "succeeded");
    assert_eq!(second["progress_percent"], 100);
    assert_eq!(second["error"], Value::Null);
    assert_eq!(second["slice_result_id"], format!("mock-result-{id}"));
}

#[tokio::test]
async fn cancel_slice_marks_cancelled() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let id = start(&t).await;
    let c = t.studio_cancel_slice(job(&id)).await.unwrap();
    assert_eq!(c["job_id"], id.as_str());
    assert_eq!(c["status"], "cancelled");
    assert_eq!(c["cancel_requested"], true);
    let s = t.studio_slice_status(job(&id)).await.unwrap();
    assert_eq!(s["status"], "cancelled");
}

#[tokio::test]
async fn cancel_of_finished_job_does_not_claim_a_cancel() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let id = slice_to_done(&t).await;
    let c = t.studio_cancel_slice(job(&id)).await.unwrap();
    assert_eq!(c["status"], "succeeded");
    assert_eq!(c["cancel_requested"], false);
}

#[tokio::test]
async fn cancel_works_while_modal_is_open() {
    // `job_cancel` is a read in the driver: no BUSY gate.
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let id = start(&t).await;
    mock.set_modal(true);
    let c = t.studio_cancel_slice(job(&id)).await.unwrap();
    assert_eq!(c["status"], "cancelled");
}

#[tokio::test]
async fn stale_job_after_user_edit_is_terminal_with_message() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let id = start(&t).await;
    mock.user_edit("layer_height", "0.3");
    let s = t.studio_slice_status(job(&id)).await.unwrap();
    assert_eq!(s["status"], "failed");
    assert_eq!(s["error"]["code"], "STALE_RESULT");
    let msg = s["message"].as_str().unwrap();
    assert!(msg.contains("invalidated by a later change"), "{msg}");
    assert!(msg.contains("slice again"), "{msg}");
}

#[tokio::test]
async fn summary_before_slice_is_invalid_argument() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let e = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
    assert!(e.message.contains("slice the plate first"), "{}", e.message);
}

#[tokio::test]
async fn summary_after_slice_has_time_and_grams() {
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(None, two_filaments(), Some(1500.0));
    mock.set_estimated_time(Some(5025.0));
    let t = tools_with(&mock);
    slice_to_done(&t).await;
    let s = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    assert_eq!(s["plate_id"], 0);
    assert_eq!(s["print_time_s"], 5025.0);
    let f = s["filaments"].as_array().unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0]["index"], 0);
    assert_eq!(f[0]["slot"], 1);
    assert_eq!(f[0]["grams"], 12.34);
    assert_eq!(f[0]["meters"], 4.5);
    assert_eq!(f[1]["grams"], 0.5);
    assert_eq!(f[1]["meters"], 0.2);
    assert_eq!(s["support_volume_mm3"], 1500.0);
    assert_eq!(s["note"], Value::Null);
    assert!(s["warnings"].as_array().unwrap().is_empty());
    assert!(s["revision"].as_u64().unwrap() > 0);
    // The export went through the driver, and the temp file is gone.
    assert_eq!(count(&mock, "export_gcode"), 1);
    assert!(mock.exported_paths().iter().all(|p| !p.exists()));
}

#[tokio::test]
async fn summary_export_goes_to_the_injected_export_dir() {
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(None, two_filaments(), None);
    let dir = test_dir();
    let t = tools_in(&mock, dir.clone());
    slice_to_done(&t).await;
    t.studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    let paths = mock.exported_paths();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].parent().unwrap(), dir.as_path());
    assert_eq!(paths[0].extension().unwrap(), "gcode");
}

#[tokio::test]
async fn summary_missing_support_volume_is_null() {
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(None, two_filaments(), None);
    let t = tools_with(&mock);
    slice_to_done(&t).await;
    let s = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    assert_eq!(s["support_volume_mm3"], Value::Null);
}

#[tokio::test]
async fn summary_without_header_has_empty_filaments_and_note() {
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(None, two_filaments(), None);
    mock.set_gcode_header(false);
    let t = tools_with(&mock);
    slice_to_done(&t).await;
    let s = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    assert!(s["filaments"].as_array().unwrap().is_empty());
    let note = s["note"].as_str().unwrap();
    assert!(note.contains("header"), "{note}");
    assert_eq!(s["print_time_s"], 1800.0);
    assert!(mock.exported_paths().iter().all(|p| !p.exists()));
}

#[tokio::test]
async fn summary_survives_a_failed_export() {
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(None, two_filaments(), None);
    let t = tools_with(&mock);
    slice_to_done(&t).await;
    mock.fail_tool(
        DriverTool::ExportGcode,
        DriverError::Busy {
            message: "export scheduled".to_string(),
        },
        1,
    );
    let s = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    assert!(s["filaments"].as_array().unwrap().is_empty());
    let note = s["note"].as_str().unwrap();
    assert!(note.contains("export"), "{note}");
    assert_eq!(s["print_time_s"], 1800.0);
    assert!(mock.exported_paths().iter().all(|p| !p.exists()));
}

#[tokio::test]
async fn summary_after_user_edit_is_invalid_argument() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    slice_to_done(&t).await;
    mock.user_edit("layer_height", "0.3");
    let e = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
}

#[tokio::test]
async fn summary_of_another_plate_is_invalid_argument() {
    let mock = Arc::new(MockDriver::new());
    mock.set_plate_count(2);
    let t = tools_with(&mock);
    slice_to_done(&t).await;
    let e = t
        .studio_get_slice_summary(summary_args(Some(1)))
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::InvalidArgument);
}

#[tokio::test]
async fn slice_with_modal_open_is_busy() {
    let mock = Arc::new(MockDriver::new());
    mock.set_modal(true);
    let t = tools_with(&mock);
    let e = t.studio_slice(slice_args(None)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
    assert_eq!(count(&mock, "slice_start"), 0);
}

#[tokio::test]
async fn second_slice_while_running_is_busy() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    start(&t).await;
    let e = t.studio_slice(slice_args(None)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
}

#[tokio::test]
async fn driver_busy_on_slice_start_maps_to_busy() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    mock.fail_tool(
        DriverTool::SliceStart,
        DriverError::Busy {
            message: "another job".to_string(),
        },
        1,
    );
    let e = t.studio_slice(slice_args(None)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
}

#[tokio::test]
async fn unknown_job_is_job_not_found() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let e = t.studio_slice_status(job("nope")).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::JobNotFound);
    let e = t.studio_cancel_slice(job("nope")).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::JobNotFound);
}

#[tokio::test]
async fn unknown_plate_is_plate_not_found() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    let e = t.studio_slice(slice_args(Some(7))).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PlateNotFound);
}

#[tokio::test]
async fn slice_other_plate_selects_it_first() {
    let mock = Arc::new(MockDriver::new());
    mock.set_plate_count(2);
    let t = tools_with(&mock);
    let r = t.studio_slice(slice_args(Some(1))).await.unwrap();
    assert_eq!(r["plate_id"], 1);
    assert_eq!(r["status"], "running");
    let calls: Vec<String> = mock
        .calls()
        .into_iter()
        .filter(|c| matches!(c.as_str(), "plate_list" | "plate_select" | "slice_start"))
        .collect();
    let select = calls.iter().position(|c| c == "plate_select").unwrap();
    let slice = calls.iter().position(|c| c == "slice_start").unwrap();
    assert!(select < slice, "{calls:?}");
    // A fresh plate_list right before slice_start, after the selection.
    assert_eq!(calls[slice - 1], "plate_list", "{calls:?}");
    assert!(slice - 1 > select, "{calls:?}");
    assert!(
        calls[..select].iter().any(|c| c == "plate_list"),
        "{calls:?}"
    );
    // The job belongs to the newly selected plate.
    let id = r["job_id"].as_str().unwrap();
    let s = t.studio_slice_status(job(id)).await.unwrap();
    assert_eq!(s["plate_id"], 1);
}

#[tokio::test]
async fn slice_of_selected_plate_does_not_select() {
    let mock = Arc::new(MockDriver::new());
    mock.set_plate_count(2);
    let t = tools_with(&mock);
    t.studio_slice(slice_args(Some(0))).await.unwrap();
    assert_eq!(count(&mock, "plate_select"), 0);
}

#[tokio::test]
async fn stale_plate_revision_is_retried_once() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    mock.fail_tool(
        DriverTool::SliceStart,
        DriverError::StaleReference {
            message: "plate revision is outdated".to_string(),
        },
        1,
    );
    let r = t.studio_slice(slice_args(None)).await.unwrap();
    assert_eq!(r["status"], "running");
    assert_eq!(count(&mock, "slice_start"), 2);
}

#[tokio::test]
async fn persistent_stale_reference_is_a_conflict() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    mock.fail_tool(
        DriverTool::SliceStart,
        DriverError::StaleReference {
            message: "plate revision is outdated".to_string(),
        },
        2,
    );
    let e = t.studio_slice(slice_args(None)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(count(&mock, "slice_start"), 2);
}

#[tokio::test]
async fn ids_come_from_the_header_not_the_position() {
    // A plate using slots 1 and 3 (0-based); slot 2 is unused (0 g), so the
    // header lists two values and `; filament: 1,3`.
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(
        None,
        vec![
            FilamentUsage {
                index: 0,
                grams: Some(0.0),
                meters: Some(0.0),
            },
            FilamentUsage {
                index: 1,
                grams: Some(12.34),
                meters: Some(4.5),
            },
            FilamentUsage {
                index: 2,
                grams: Some(0.0),
                meters: Some(0.0),
            },
            FilamentUsage {
                index: 3,
                grams: Some(0.5),
                meters: Some(0.2),
            },
        ],
        None,
    );
    let t = tools_with(&mock);
    slice_to_done(&t).await;
    let s = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    let f = s["filaments"].as_array().unwrap();
    assert_eq!(f.len(), 2, "{f:?}");
    assert_eq!(f[0]["index"], 1);
    assert_eq!(f[0]["slot"], 2);
    assert_eq!(f[0]["grams"], 12.34);
    assert_eq!(f[1]["index"], 3);
    assert_eq!(f[1]["slot"], 4);
    assert_eq!(f[1]["grams"], 0.5);
    assert_eq!(f[1]["meters"], 0.2);
    assert_eq!(s["note"], Value::Null);
}

#[tokio::test]
async fn stale_export_files_are_swept_before_an_export() {
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(None, two_filaments(), None);
    let dir = test_dir();
    let stale = dir.join("summary-1-1.gcode");
    let fresh = dir.join("summary-1-2.gcode");
    let other = dir.join("keep-me.gcode");
    for p in [&stale, &fresh, &other] {
        std::fs::write(p, "x").unwrap();
    }
    let old = std::time::SystemTime::now() - Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let t = tools_in(&mock, dir.clone());
    slice_to_done(&t).await;
    t.studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    assert!(!stale.exists(), "stale export was not swept");
    assert!(
        fresh.exists(),
        "a recent file may belong to a running export"
    );
    assert!(other.exists(), "only summary-*.gcode files are ours");
}

#[tokio::test]
async fn missing_export_dir_is_a_note_not_an_error() {
    let mock = Arc::new(MockDriver::new());
    mock.set_summary_stats(None, two_filaments(), None);
    let missing = test_dir().join("does-not-exist");
    let t = tools_in(&mock, missing);
    slice_to_done(&t).await;
    let s = t
        .studio_get_slice_summary(summary_args(None))
        .await
        .unwrap();
    assert!(s["filaments"].as_array().unwrap().is_empty());
    assert!(s["note"].as_str().unwrap().contains("export directory"));
    assert_eq!(count(&mock, "export_gcode"), 0);
}

#[tokio::test]
async fn plate_selection_changed_has_its_own_message() {
    let mock = Arc::new(MockDriver::new());
    let t = tools_with(&mock);
    mock.fail_tool(
        DriverTool::SliceStart,
        DriverError::InvalidParams {
            message: "Only the selected plate is supported; select it explicitly first".to_string(),
        },
        1,
    );
    let e = t.studio_slice(slice_args(None)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::RevisionConflict);
    assert!(
        e.message.contains("plate selection changed"),
        "{}",
        e.message
    );
    assert_eq!(e.details["reason"], "plate_selection_changed");
}

// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::DriverError;
use bamburino_core::driver::mock::{MockArrange, MockDriver};
use bamburino_core::driver::tool::DriverTool;
use bamburino_core::errors::ToolErrorCode;
use bamburino_core::tools::{ArrangeArgs, JobArgs, ToolConfig, Tools};

fn tools_with(mock: &Arc<MockDriver>, timeout: Duration) -> Tools {
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
    .with_job_wait(Duration::from_millis(1), timeout)
}

/// Two plates; the default cube `obj-1` on plate 0 and `obj-2` on plate 1.
fn two_plates() -> Arc<MockDriver> {
    let m = Arc::new(MockDriver::new());
    m.set_plate_count(2);
    m.add_object("obj-2", "slide", 1);
    m
}

async fn revision(t: &Tools) -> u64 {
    t.studio_get_state().await.unwrap()["revision"]
        .as_u64()
        .unwrap()
}

fn arrange(plate_index: u32, rev: u64) -> ArrangeArgs {
    ArrangeArgs {
        plate_index,
        expected_revision: rev,
    }
}

fn count(mock: &MockDriver, name: &str) -> usize {
    mock.calls().iter().filter(|c| *c == name).count()
}

const LONG: Duration = Duration::from_secs(2);

#[tokio::test]
async fn arrange_selects_the_plate_then_arranges_and_reports_moves() {
    let mock = two_plates();
    mock.set_arrange(MockArrange::Shift([5.0, 0.0, 0.0]));
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    let v = t.studio_arrange(arrange(1, rev)).await.unwrap();
    let calls = mock.calls();
    let pos = |n: &str| calls.iter().position(|c| c == n).unwrap();
    assert!(pos("plate_select") < pos("arrange_start"));
    assert!(pos("arrange_start") < pos("job_get"));
    assert_eq!(v["status"], "succeeded");
    assert_eq!(v["scope"], "project");
    assert_eq!(v["plate_index"], 1);
    let moved = v["moved"].as_array().unwrap();
    assert_eq!(moved.len(), 2);
    for m in moved {
        let (f, to) = (&m["from_mm"], &m["to_mm"]);
        assert_eq!(to[0].as_f64().unwrap() - f[0].as_f64().unwrap(), 5.0);
        assert!(m["object_id"].is_string() && m["instance_id"].is_string());
        assert!(m["plate_index"].is_number());
    }
    assert!(v["moved_between_plates"].as_array().unwrap().is_empty());
    assert!(v["warning"].is_null());
}

#[tokio::test]
async fn arrange_does_not_reselect_the_selected_plate() {
    let mock = two_plates();
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    t.studio_arrange(arrange(0, rev)).await.unwrap();
    assert_eq!(count(&mock, "plate_select"), 0);
    assert_eq!(count(&mock, "arrange_start"), 1);
}

#[tokio::test]
async fn arrange_reports_cross_plate_moves() {
    let mock = two_plates();
    mock.set_arrange(MockArrange::ShiftAndSwapPlates);
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    let v = t.studio_arrange(arrange(0, rev)).await.unwrap();
    let between = v["moved_between_plates"].as_array().unwrap();
    assert_eq!(between.len(), 2);
    let cube = between.iter().find(|m| m["object_id"] == "obj-1").unwrap();
    assert_eq!(cube["from_plate_index"], 0);
    assert_eq!(cube["to_plate_index"], 1);
    assert!(v["warning"].as_str().unwrap().contains("plate"));
    assert_eq!(v["scope"], "project");
}

#[tokio::test]
async fn arrange_failed_job_is_driver_error_with_the_job_message() {
    let mock = two_plates();
    mock.set_arrange(MockArrange::Fail("boom".into()));
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    let e = t.studio_arrange(arrange(0, rev)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::DriverError);
    assert_eq!(e.details["driver_error"], "boom");
    assert!(e.details["job_id"].is_string());
    assert_eq!(e.details["status"], "failed");
}

#[tokio::test]
async fn arrange_timeout_returns_running_with_the_job_id_not_an_error() {
    let mock = two_plates();
    let t = tools_with(&mock, Duration::from_millis(0));
    let rev = revision(&t).await;
    let v = t.studio_arrange(arrange(0, rev)).await.unwrap();
    assert_eq!(v["status"], "running");
    let w = v["warning"].as_str().unwrap();
    assert!(w.contains("studio_slice_status") && w.contains("studio_cancel_slice"));
    let id = v["job_id"].as_str().unwrap().to_string();
    let c = t.studio_cancel_slice(JobArgs { job_id: id }).await.unwrap();
    assert_eq!(c["cancel_requested"], true);
}

#[tokio::test]
async fn arrange_job_is_cancellable_with_studio_cancel_slice() {
    let mock = two_plates();
    let t = tools_with(&mock, Duration::from_millis(0));
    let rev = revision(&t).await;
    let v = t.studio_arrange(arrange(0, rev)).await.unwrap();
    let id = v["job_id"].as_str().unwrap().to_string();
    let c = t
        .studio_cancel_slice(JobArgs { job_id: id.clone() })
        .await
        .unwrap();
    assert_eq!(c["cancel_requested"], true);
    assert_eq!(c["status"], "cancelled");
    let s = t.studio_slice_status(JobArgs { job_id: id }).await.unwrap();
    assert_eq!(s["status"], "cancelled");
}

#[tokio::test]
async fn arrange_busy_when_a_modal_is_open_and_unknown_plate_is_plate_not_found() {
    let mock = two_plates();
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    let e = t.studio_arrange(arrange(9, rev)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::PlateNotFound);
    assert_eq!(count(&mock, "arrange_start"), 0);
    mock.set_modal(true);
    let e = t.studio_arrange(arrange(0, rev)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
    assert_eq!(count(&mock, "arrange_start"), 0);
}

#[tokio::test]
async fn arrange_counts_two_snapshots_when_it_selects_and_one_when_it_does_not() {
    let mock = two_plates();
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    // Selects plate 1: two snapshots (plate_select + arrange_start).
    let v = t.studio_arrange(arrange(1, rev)).await.unwrap();
    assert_eq!(v["revision"].as_u64().unwrap(), revision(&t).await);
    // Plate 1 is selected now: one snapshot (arrange_start).
    let next = v["revision"].as_u64().unwrap();
    let v2 = t.studio_arrange(arrange(1, next)).await.unwrap();
    assert_eq!(v2["revision"].as_u64().unwrap(), revision(&t).await);
    assert_eq!(count(&mock, "plate_select"), 1);
}

#[tokio::test]
async fn arrange_reports_the_selection_side_effect() {
    let mock = two_plates();
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    let v = t.studio_arrange(arrange(1, rev)).await.unwrap();
    assert_eq!(v["selected_plate_index"], 1);
    assert_eq!(v["previous_plate_index"], 0);
    assert_eq!(v["selected_plate_changed"], true);
    assert!(v["outcome"].is_null());
    let next = v["revision"].as_u64().unwrap();
    let v2 = t.studio_arrange(arrange(1, next)).await.unwrap();
    assert_eq!(v2["selected_plate_changed"], false);
}

#[tokio::test]
async fn arrange_job_poll_failure_keeps_the_job_id_and_verifies_snapshots() {
    let mock = two_plates();
    mock.fail_tool(DriverTool::JobGet, DriverError::Timeout { after_ms: 5 }, 1);
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    let e = t.studio_arrange(arrange(1, rev)).await.unwrap_err();
    // Not a snapshot conflict: the pipeline verified the two snapshots.
    assert_ne!(e.code, ToolErrorCode::RevisionConflict);
    assert_eq!(e.details["outcome"], "job_running_unobserved");
    let id = e.details["job_id"].as_str().unwrap().to_string();
    assert!(id.ends_with(":arrange"));
    assert!(e.details["revision"].is_number());
    // The caller can still poll the job.
    let s = t.studio_slice_status(JobArgs { job_id: id }).await.unwrap();
    assert_eq!(s["status"], "running");
}

#[tokio::test]
async fn arrange_timeout_result_names_the_outcome_and_blocks_changes_as_busy() {
    let mock = two_plates();
    let t = tools_with(&mock, Duration::from_millis(0));
    let rev = revision(&t).await;
    let v = t.studio_arrange(arrange(0, rev)).await.unwrap();
    assert_eq!(v["outcome"], "timeout_job_running");
    assert!(v["warning"].as_str().unwrap().contains("studio_get_state"));
    // The job still runs: another change is refused as BUSY.
    let next = v["revision"].as_u64().unwrap();
    let e = t.studio_arrange(arrange(0, next)).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
}

#[tokio::test]
async fn wait_job_is_bounded_by_the_timeout() {
    // Zero timeout: no poll. Long timeout: polls until the job ends (the
    // mock finishes an arrange job at its second poll).
    let mock = two_plates();
    let t = tools_with(&mock, Duration::from_millis(0));
    let rev = revision(&t).await;
    t.studio_arrange(arrange(0, rev)).await.unwrap();
    assert_eq!(count(&mock, "job_get"), 0);
    let mock = two_plates();
    let t = tools_with(&mock, LONG);
    let rev = revision(&t).await;
    t.studio_arrange(arrange(0, rev)).await.unwrap();
    assert_eq!(count(&mock, "job_get"), 2);
}
